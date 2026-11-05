use zendiloos_kernel::Timestamp;

/// One sample of the live metrics, taken every few seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsSample {
    pub at: Timestamp,
    /// Busy share of all CPUs since the previous sample, 0 to 100.
    pub cpu_percent: f32,
    /// 1, 5 and 15 minute load averages.
    pub load_average: [f64; 3],
    pub memory: MemoryUsage,
    /// Physical network interfaces only.
    pub network: Vec<InterfaceRate>,
    /// Whole disks and arrays, no partitions.
    pub disks: Vec<DiskRate>,
    pub temperatures: Vec<Temperature>,
    pub uptime_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceRate {
    pub name: String,
    pub received_bytes_per_second: u64,
    pub sent_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskRate {
    pub name: String,
    pub read_bytes_per_second: u64,
    pub written_bytes_per_second: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Temperature {
    pub label: String,
    pub celsius: f32,
}
