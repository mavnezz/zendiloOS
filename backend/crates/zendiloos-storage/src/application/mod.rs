//! Use cases of the Storage context and the ports they need.

pub mod commands;
pub mod enclosure;
mod error;
pub mod inventory;
pub mod monitor;
pub mod ports;
pub mod queries;
pub mod removable;
pub mod self_tests;

use std::sync::Arc;

pub use error::StorageError;
use ports::{
    ArraySource, BlockDeviceSource, DiskInventoryStore, DiskLocator, EnclosureCatalog, EnclosureStore,
    MachineSource, PlanTokens, PoolAdmin, RemovableHost, SelfTestPlanStore, SmartSource, VolumeOccupants,
    ZfsSource,
};
use zendiloos_jobs::Jobs;
use zendiloos_kernel::{AuditLog, Clock};
use zendiloos_notify::Notifier;

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct StoragePorts {
    pub devices: Arc<dyn BlockDeviceSource>,
    pub smart: Arc<dyn SmartSource>,
    pub arrays: Arc<dyn ArraySource>,
    pub zfs: Arc<dyn ZfsSource>,
    pub admin: Arc<dyn PoolAdmin>,
    pub jobs: Jobs,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
    pub tokens: Arc<dyn PlanTokens>,
    pub notifier: Notifier,
    pub self_test_plans: Arc<dyn SelfTestPlanStore>,
    pub occupants: Arc<dyn VolumeOccupants>,
    pub removable: Arc<dyn RemovableHost>,
    pub machine: Arc<dyn MachineSource>,
    pub catalog: Arc<dyn EnclosureCatalog>,
    pub enclosure: Arc<dyn EnclosureStore>,
    pub locator: Arc<dyn DiskLocator>,
    pub inventory: Arc<dyn DiskInventoryStore>,
}

/// All use cases of this context.
#[derive(Clone)]
pub struct StorageUseCases {
    pub list_disks: queries::ListDisks,
    pub get_smart_report: queries::GetSmartReport,
    pub list_arrays: queries::ListArrays,
    pub list_volumes: queries::ListVolumes,
    pub list_pools: queries::ListPools,
    pub overview: queries::GetStorageOverview,
    pub plan_pool: commands::PlanPool,
    pub create_pool: commands::CreatePool,
    pub destroy_pool: commands::DestroyPool,
    pub plan_replace: commands::PlanReplace,
    pub replace_disk: commands::ReplaceDisk,
    pub scrub_pool: commands::ScrubPool,
    pub stop_scrub: commands::StopScrub,
    pub self_test: commands::SelfTest,
    pub self_test_schedule: self_tests::SelfTestSchedule,
    pub monitor: Arc<monitor::StorageMonitor>,
    pub removable: Arc<removable::RemovableDrives>,
    pub enclosure: enclosure::Enclosures,
    pub locate: enclosure::Locator,
    pub inventory: inventory::DiskInventory,
}

impl StorageUseCases {
    pub fn new(ports: StoragePorts) -> Self {
        let removable = Arc::new(removable::RemovableDrives::new(
            ports.devices.clone(),
            ports.removable,
            ports.audit.clone(),
            ports.clock.clone(),
            ports.notifier.clone(),
        ));
        let monitor = Arc::new(monitor::StorageMonitor::new(
            ports.devices.clone(),
            ports.zfs.clone(),
            ports.smart.clone(),
            ports.inventory.clone(),
            ports.notifier,
            ports.clock.clone(),
        ));
        let overview = queries::GetStorageOverview::new(
            ports.devices.clone(),
            ports.zfs.clone(),
            Arc::clone(&monitor),
            ports.clock.clone(),
        );
        let self_test_schedule = self_tests::SelfTestSchedule::new(
            ports.devices.clone(),
            ports.smart.clone(),
            ports.self_test_plans,
            ports.audit.clone(),
            ports.clock.clone(),
        );
        let enclosure = enclosure::Enclosures::new(
            ports.devices.clone(),
            ports.zfs.clone(),
            ports.machine,
            ports.catalog,
            ports.enclosure,
            Arc::clone(&monitor),
            ports.audit.clone(),
            ports.clock.clone(),
        );
        let locate = enclosure::Locator::new(
            ports.devices.clone(),
            ports.locator,
            ports.audit.clone(),
            ports.clock.clone(),
        );
        let inventory = inventory::DiskInventory::new(
            ports.devices.clone(),
            ports.inventory,
            ports.audit.clone(),
            ports.clock.clone(),
        );
        let context = Arc::new(commands::Context::new(
            ports.devices.clone(),
            ports.zfs.clone(),
            ports.admin,
            ports.jobs,
            ports.audit,
            ports.clock.clone(),
            ports.tokens,
            ports.smart.clone(),
            ports.occupants,
        ));
        Self {
            plan_pool: commands::PlanPool::new(Arc::clone(&context)),
            create_pool: commands::CreatePool::new(Arc::clone(&context)),
            destroy_pool: commands::DestroyPool::new(Arc::clone(&context)),
            plan_replace: commands::PlanReplace::new(Arc::clone(&context)),
            replace_disk: commands::ReplaceDisk::new(Arc::clone(&context)),
            scrub_pool: commands::ScrubPool::new(Arc::clone(&context)),
            stop_scrub: commands::StopScrub::new(Arc::clone(&context)),
            self_test: commands::SelfTest::new(context),
            self_test_schedule,
            monitor,
            removable,
            enclosure,
            locate,
            inventory,
            overview,
            list_disks: queries::ListDisks::new(ports.devices.clone(), ports.zfs.clone()),
            get_smart_report: queries::GetSmartReport::new(ports.devices.clone(), ports.smart),
            list_arrays: queries::ListArrays::new(ports.arrays),
            list_volumes: queries::ListVolumes::new(ports.devices.clone(), ports.zfs.clone()),
            list_pools: queries::ListPools::new(ports.devices, ports.zfs, ports.clock.clone()),
        }
    }
}
