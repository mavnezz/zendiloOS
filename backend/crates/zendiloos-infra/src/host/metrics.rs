//! Live metrics of the running host: sysinfo for CPU, memory, network and
//! sensors, `/proc/diskstats` for disk I/O.

use std::fs;
use std::path::Path;
use std::time::Instant;

use sysinfo::{Components, CpuRefreshKind, MemoryRefreshKind, Networks, RefreshKind, System};
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_system::domain::{DiskRate, InterfaceRate, MemoryUsage, MetricsSample, Temperature};

/// Takes samples; keeps the previous counters to turn them into rates.
pub struct HostMetrics {
    system: System,
    networks: Networks,
    components: Components,
    previous_disks: Vec<DiskCounters>,
    previous_at: Instant,
}

impl HostMetrics {
    pub fn new() -> Result<Self, PortError> {
        let read = |what| move |error| PortError::new(what, error);
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram()),
        )
        .map_err(read("CPU and memory information"))?;
        Ok(Self {
            system,
            networks: Networks::new_with_refreshed_list().map_err(read("network statistics"))?,
            components: Components::new_with_refreshed_list().map_err(read("sensors"))?,
            previous_disks: read_disk_counters(),
            previous_at: Instant::now(),
        })
    }

    /// Rates are averages since the previous call (or since `new`).
    pub fn sample(&mut self, at: Timestamp) -> MetricsSample {
        let now = Instant::now();
        let seconds = now.duration_since(self.previous_at).as_secs_f64().max(0.001);
        self.previous_at = now;

        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        self.components.refresh(true);
        let disks = read_disk_counters();
        let disk_rates = disk_rates(&self.previous_disks, &disks, seconds);
        self.previous_disks = disks;

        let per_second = |bytes: u64| (bytes as f64 / seconds).round() as u64;
        let load = System::load_average().map(|l| [l.one, l.five, l.fifteen]).unwrap_or_default();
        MetricsSample {
            at,
            cpu_percent: self.system.global_cpu_usage(),
            load_average: load,
            memory: MemoryUsage {
                total_bytes: self.system.total_memory(),
                used_bytes: self.system.used_memory(),
                available_bytes: self.system.available_memory(),
            },
            network: self
                .networks
                .iter()
                .filter(|(name, _)| is_physical_interface(name))
                .map(|(name, data)| InterfaceRate {
                    name: name.to_string_lossy().into_owned(),
                    received_bytes_per_second: per_second(data.received()),
                    sent_bytes_per_second: per_second(data.transmitted()),
                })
                .collect(),
            disks: disk_rates,
            temperatures: self
                .components
                .iter()
                .filter_map(|c| Some(Temperature { label: c.label().to_owned(), celsius: c.temperature()? }))
                .collect(),
            uptime_seconds: System::uptime().unwrap_or_default(),
        }
    }
}

/// Interfaces backed by a device: no loopback, bridges, veths or tunnels.
fn is_physical_interface(name: &std::ffi::OsStr) -> bool {
    Path::new("/sys/class/net").join(name).join("device").exists()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiskCounters {
    name: String,
    sectors_read: u64,
    sectors_written: u64,
}

fn read_disk_counters() -> Vec<DiskCounters> {
    fs::read_to_string("/proc/diskstats")
        .map(|text| parse_diskstats(&text, |name| Path::new("/sys/block").join(name).exists()))
        .unwrap_or_default()
}

/// Whole disks and arrays from `/proc/diskstats`; `is_block_device` tells
/// them from partitions (only whole devices appear in `/sys/block`).
fn parse_diskstats(text: &str, is_block_device: impl Fn(&str) -> bool) -> Vec<DiskCounters> {
    const VIRTUAL: [&str; 4] = ["loop", "ram", "zram", "sr"];
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let name = *fields.get(2)?;
            if VIRTUAL.iter().any(|prefix| name.starts_with(prefix)) || !is_block_device(name) {
                return None;
            }
            Some(DiskCounters {
                name: name.to_owned(),
                sectors_read: fields.get(5)?.parse().ok()?,
                sectors_written: fields.get(9)?.parse().ok()?,
            })
        })
        .collect()
}

/// `/proc/diskstats` counts 512-byte sectors, whatever the disk's sector size.
fn disk_rates(previous: &[DiskCounters], current: &[DiskCounters], seconds: f64) -> Vec<DiskRate> {
    current
        .iter()
        .map(|disk| {
            let before = previous.iter().find(|p| p.name == disk.name);
            let delta = |now: u64, then: Option<u64>| now.saturating_sub(then.unwrap_or(now)) * 512;
            let rate = |bytes: u64| (bytes as f64 / seconds).round() as u64;
            DiskRate {
                name: disk.name.clone(),
                read_bytes_per_second: rate(delta(disk.sectors_read, before.map(|b| b.sectors_read))),
                written_bytes_per_second: rate(delta(
                    disk.sectors_written,
                    before.map(|b| b.sectors_written),
                )),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISKSTATS: &str = "\
 259       0 nvme0n1 120 0 4000 10 50 0 2000 5 0 20 15 0 0 0 0 0 0
 259       1 nvme0n1p1 100 0 3000 8 40 0 1000 4 0 18 12 0 0 0 0 0 0
   8       0 sda 10 0 80 1 0 0 0 0 0 1 1 0 0 0 0 0 0
   7       0 loop0 5 0 10 0 0 0 0 0 0 0 0 0 0 0 0 0 0
   9     127 md127 30 0 600 0 10 0 160 0 0 0 0 0 0 0 0 0 0
";

    fn whole_disks(name: &str) -> bool {
        !name.contains('p') || name == "loop0"
    }

    #[test]
    fn reads_whole_disks_and_arrays_only() {
        let disks = parse_diskstats(DISKSTATS, whole_disks);
        let names: Vec<&str> = disks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["nvme0n1", "sda", "md127"]);
        assert_eq!((disks[0].sectors_read, disks[0].sectors_written), (4000, 2000));
    }

    #[test]
    fn turns_sector_counts_into_byte_rates() {
        let before = parse_diskstats(DISKSTATS, whole_disks);
        let mut after = before.clone();
        after[0].sectors_read += 2048; // 1 MiB
        after[0].sectors_written += 4096; // 2 MiB

        let rates = disk_rates(&before, &after, 2.0);

        assert_eq!(rates[0].read_bytes_per_second, 512 * 1024);
        assert_eq!(rates[0].written_bytes_per_second, 1024 * 1024);
        assert_eq!(rates[1].read_bytes_per_second, 0);
    }

    #[test]
    fn a_new_disk_starts_at_zero() {
        let after = parse_diskstats(DISKSTATS, whole_disks);
        let rates = disk_rates(&[], &after, 2.0);
        assert!(rates.iter().all(|r| r.read_bytes_per_second == 0 && r.written_bytes_per_second == 0));
    }

    #[test]
    fn samples_the_running_host() {
        let mut metrics = HostMetrics::new().unwrap();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let sample = metrics.sample(Timestamp::from_unix_seconds(1));
        assert!((0.0..=100.0).contains(&sample.cpu_percent));
        assert!(sample.memory.total_bytes > 0);
        assert!(sample.memory.used_bytes <= sample.memory.total_bytes);
    }
}
