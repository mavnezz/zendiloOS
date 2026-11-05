use std::fs;

use async_trait::async_trait;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use zendiloos_kernel::PortError;
use zendiloos_system::application::ports::SystemInfoProvider;
use zendiloos_system::domain::SystemInfo;

use super::os_release;

/// [`SystemInfoProvider`] for the host this process runs on.
#[derive(Debug, Default)]
pub struct HostSystemInfo;

/// os-release(5): `/etc/os-release` takes precedence, `/usr/lib/os-release`
/// is the fallback.
const OS_RELEASE_PATHS: [&str; 2] = ["/etc/os-release", "/usr/lib/os-release"];

#[async_trait]
impl SystemInfoProvider for HostSystemInfo {
    async fn system_info(&self) -> Result<SystemInfo, PortError> {
        // Reads a handful of small files; keep it off the async workers anyway.
        tokio::task::spawn_blocking(read_system_info)
            .await
            .map_err(|error| PortError::new("system information", error))?
    }
}

fn read<T>(what: &'static str, result: Result<T, sysinfo::Error>) -> Result<T, PortError> {
    result.map_err(|error| PortError::new(what, error))
}

fn read_system_info() -> Result<SystemInfo, PortError> {
    let system = read(
        "CPU and memory information",
        System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        ),
    )?;

    Ok(SystemInfo {
        hostname: read("hostname", System::host_name())?,
        os_name: OS_RELEASE_PATHS
            .iter()
            .find_map(|path| fs::read_to_string(path).ok())
            .and_then(|content| os_release::pretty_name(&content)),
        kernel_release: read("kernel release", System::kernel_version())?,
        architecture: System::cpu_arch(),
        cpu_model: system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().trim().to_owned())
            .filter(|brand| !brand.is_empty()),
        cpu_threads: system.cpus().len(),
        memory_total_bytes: system.total_memory(),
        uptime_seconds: read("uptime", System::uptime())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs against the machine executing the tests; checks plausibility only.
    #[tokio::test]
    async fn reads_the_running_host() {
        let info = HostSystemInfo.system_info().await.expect("host system information");
        assert!(!info.hostname.is_empty());
        assert!(!info.kernel_release.is_empty());
        assert!(!info.architecture.is_empty());
        assert!(info.cpu_threads > 0);
        assert!(info.memory_total_bytes > 0);
    }
}
