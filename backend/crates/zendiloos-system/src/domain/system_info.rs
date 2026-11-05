/// Static facts about the host, plus its uptime. A value object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub hostname: String,
    /// `PRETTY_NAME` from os-release, e.g. `Debian GNU/Linux 13 (trixie)`.
    pub os_name: Option<String>,
    /// Kernel release, e.g. `6.12.48+deb13-amd64`.
    pub kernel_release: String,
    /// Machine architecture, e.g. `x86_64` or `aarch64`.
    pub architecture: String,
    /// CPU model as reported by the kernel, e.g. `Intel(R) N100`.
    pub cpu_model: Option<String>,
    /// Logical CPUs (hardware threads).
    pub cpu_threads: usize,
    pub memory_total_bytes: u64,
    pub uptime_seconds: u64,
}
