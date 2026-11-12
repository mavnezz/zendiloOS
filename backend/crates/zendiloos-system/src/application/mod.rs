//! Use cases of the System context and the ports they need.

pub mod commands;
pub mod ports;
pub mod queries;

use std::sync::Arc;

use zendiloos_kernel::{AuditLog, Clock, RunningWork};

use ports::{
    AuditReader, HostNameControl, HostNameObserver, JournalReader, ListeningPorts, LiveMetrics, PowerControl,
    SshControl, SystemInfoProvider, TimeControl, WebTlsControl,
};

/// Adapters for every port of this context, plugged in by the composition
/// root.
#[derive(Clone)]
pub struct SystemPorts {
    pub info: Arc<dyn SystemInfoProvider>,
    pub metrics: Arc<dyn LiveMetrics>,
    pub power: Arc<dyn PowerControl>,
    /// What runs in the background, which a reboot waits for.
    pub work: Arc<dyn RunningWork>,
    pub time: Arc<dyn TimeControl>,
    pub host_name: Arc<dyn HostNameControl>,
    pub host_name_observer: Arc<dyn HostNameObserver>,
    pub ssh: Arc<dyn SshControl>,
    pub listening: Arc<dyn ListeningPorts>,
    pub tls: Arc<dyn WebTlsControl>,
    pub journal: Arc<dyn JournalReader>,
    pub audit: Arc<dyn AuditLog>,
    pub audit_reader: Arc<dyn AuditReader>,
    pub clock: Arc<dyn Clock>,
}

/// All use cases of this context.
#[derive(Clone)]
pub struct SystemUseCases {
    pub get_info: queries::GetSystemInfo,
    pub watch_metrics: queries::WatchMetrics,
    pub request_power_action: commands::RequestPowerAction,
    pub get_time: queries::GetTime,
    pub set_time: commands::SetTime,
    pub get_host_name: queries::GetHostName,
    pub set_host_name: commands::SetHostName,
    pub get_ssh: queries::GetSsh,
    pub set_ssh: commands::SetSsh,
    pub get_tls: queries::GetTls,
    pub set_tls: commands::SetTls,
    pub read_journal: queries::ReadJournal,
    pub read_audit_log: queries::ReadAuditLog,
}

impl SystemUseCases {
    pub fn new(ports: SystemPorts) -> Self {
        Self {
            get_info: queries::GetSystemInfo::new(ports.info),
            watch_metrics: queries::WatchMetrics::new(ports.metrics),
            request_power_action: commands::RequestPowerAction::new(
                ports.power,
                ports.work,
                ports.audit.clone(),
                ports.clock.clone(),
            ),
            get_time: queries::GetTime::new(ports.time.clone()),
            set_time: commands::SetTime::new(ports.time, ports.audit.clone(), ports.clock.clone()),
            get_host_name: queries::GetHostName::new(ports.host_name.clone()),
            set_host_name: commands::SetHostName::new(
                ports.host_name,
                ports.host_name_observer,
                ports.audit.clone(),
                ports.clock.clone(),
            ),
            get_ssh: queries::GetSsh::new(ports.ssh.clone()),
            set_ssh: commands::SetSsh::new(
                ports.ssh,
                ports.listening,
                ports.audit.clone(),
                ports.clock.clone(),
            ),
            get_tls: queries::GetTls::new(ports.tls.clone()),
            set_tls: commands::SetTls::new(ports.tls, ports.audit, ports.clock),
            read_journal: queries::ReadJournal::new(ports.journal),
            read_audit_log: queries::ReadAuditLog::new(ports.audit_reader),
        }
    }
}
