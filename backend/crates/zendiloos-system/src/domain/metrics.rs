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

/// How many samples the server keeps: six minutes at one every two seconds,
/// so charts are not empty when they are opened.
pub const HISTORY_SAMPLES: usize = 180;

/// The last minutes of the host's load as series for charts, oldest first;
/// rates are summed over all interfaces and disks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetricsHistory {
    pub at: Vec<Timestamp>,
    pub cpu_percent: Vec<f32>,
    pub memory_used_bytes: Vec<u64>,
    pub received_bytes_per_second: Vec<u64>,
    pub sent_bytes_per_second: Vec<u64>,
    pub read_bytes_per_second: Vec<u64>,
    pub written_bytes_per_second: Vec<u64>,
}

impl MetricsHistory {
    pub fn of(samples: &[MetricsSample]) -> Self {
        let mut history = Self::default();
        for sample in samples {
            history.at.push(sample.at);
            history.cpu_percent.push(sample.cpu_percent);
            history.memory_used_bytes.push(sample.memory.used_bytes);
            history
                .received_bytes_per_second
                .push(sample.network.iter().map(|n| n.received_bytes_per_second).sum());
            history.sent_bytes_per_second.push(sample.network.iter().map(|n| n.sent_bytes_per_second).sum());
            history.read_bytes_per_second.push(sample.disks.iter().map(|d| d.read_bytes_per_second).sum());
            history
                .written_bytes_per_second
                .push(sample.disks.iter().map(|d| d.written_bytes_per_second).sum());
        }
        history
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    #[test]
    fn sums_rates_per_sample_oldest_first() {
        let sample = |at: i64, cpu: f32, eth0: u64, eth1: u64| MetricsSample {
            at: Timestamp::from_unix_seconds(at),
            cpu_percent: cpu,
            load_average: [0.0; 3],
            memory: MemoryUsage { total_bytes: 8, used_bytes: 2, available_bytes: 6 },
            network: vec![
                InterfaceRate {
                    name: "eth0".into(),
                    received_bytes_per_second: eth0,
                    sent_bytes_per_second: 1,
                },
                InterfaceRate {
                    name: "eth1".into(),
                    received_bytes_per_second: eth1,
                    sent_bytes_per_second: 1,
                },
            ],
            disks: vec![DiskRate {
                name: "sda".into(),
                read_bytes_per_second: 7,
                written_bytes_per_second: 3,
            }],
            temperatures: vec![],
            uptime_seconds: 1,
        };

        let history = MetricsHistory::of(&[sample(10, 5.0, 100, 20), sample(12, 50.0, 0, 30)]);

        assert_eq!(history.at, [Timestamp::from_unix_seconds(10), Timestamp::from_unix_seconds(12)]);
        assert_eq!(history.cpu_percent, [5.0, 50.0]);
        assert_eq!(history.received_bytes_per_second, [120, 30]);
        assert_eq!(history.sent_bytes_per_second, [2, 2]);
        assert_eq!((history.read_bytes_per_second[0], history.written_bytes_per_second[1]), (7, 3));
    }
}
