//! Use cases of the Storage context and the ports they need.

pub mod commands;
mod error;
pub mod monitor;
pub mod ports;
pub mod queries;

use std::sync::Arc;

pub use error::StorageError;
use ports::{ArraySource, BlockDeviceSource, PlanTokens, PoolAdmin, SmartSource, ZfsSource};
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
}

/// All use cases of this context.
#[derive(Clone)]
pub struct StorageUseCases {
    pub list_disks: queries::ListDisks,
    pub get_smart_report: queries::GetSmartReport,
    pub list_arrays: queries::ListArrays,
    pub list_volumes: queries::ListVolumes,
    pub list_pools: queries::ListPools,
    pub plan_pool: commands::PlanPool,
    pub create_pool: commands::CreatePool,
    pub destroy_pool: commands::DestroyPool,
    pub plan_replace: commands::PlanReplace,
    pub replace_disk: commands::ReplaceDisk,
    pub scrub_pool: commands::ScrubPool,
    pub monitor: Arc<monitor::StorageMonitor>,
}

impl StorageUseCases {
    pub fn new(ports: StoragePorts) -> Self {
        let monitor = Arc::new(monitor::StorageMonitor::new(
            ports.devices.clone(),
            ports.zfs.clone(),
            ports.smart.clone(),
            ports.notifier,
            ports.clock.clone(),
        ));
        let context = Arc::new(commands::Context::new(
            ports.devices.clone(),
            ports.zfs.clone(),
            ports.admin,
            ports.jobs,
            ports.audit,
            ports.clock,
            ports.tokens,
        ));
        Self {
            plan_pool: commands::PlanPool::new(Arc::clone(&context)),
            create_pool: commands::CreatePool::new(Arc::clone(&context)),
            destroy_pool: commands::DestroyPool::new(Arc::clone(&context)),
            plan_replace: commands::PlanReplace::new(Arc::clone(&context)),
            replace_disk: commands::ReplaceDisk::new(Arc::clone(&context)),
            scrub_pool: commands::ScrubPool::new(context),
            monitor,
            list_disks: queries::ListDisks::new(ports.devices.clone(), ports.zfs.clone()),
            get_smart_report: queries::GetSmartReport::new(ports.devices.clone(), ports.smart),
            list_arrays: queries::ListArrays::new(ports.arrays),
            list_volumes: queries::ListVolumes::new(ports.devices.clone(), ports.zfs.clone()),
            list_pools: queries::ListPools::new(ports.devices, ports.zfs),
        }
    }
}
