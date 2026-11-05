//! Use cases of the System context and the ports they need.

pub mod commands;
pub mod ports;
pub mod queries;

use std::sync::Arc;

use zendiloos_kernel::{AuditLog, Clock};

use ports::{LiveMetrics, PowerControl, SystemInfoProvider};

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct SystemPorts {
    pub info: Arc<dyn SystemInfoProvider>,
    pub metrics: Arc<dyn LiveMetrics>,
    pub power: Arc<dyn PowerControl>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

/// All use cases of this context.
#[derive(Clone)]
pub struct SystemUseCases {
    pub get_info: queries::GetSystemInfo,
    pub watch_metrics: queries::WatchMetrics,
    pub request_power_action: commands::RequestPowerAction,
}

impl SystemUseCases {
    pub fn new(ports: SystemPorts) -> Self {
        Self {
            get_info: queries::GetSystemInfo::new(ports.info),
            watch_metrics: queries::WatchMetrics::new(ports.metrics),
            request_power_action: commands::RequestPowerAction::new(ports.power, ports.audit, ports.clock),
        }
    }
}
