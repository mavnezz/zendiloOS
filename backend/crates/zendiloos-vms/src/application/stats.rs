//! What the VMs use, every few seconds while someone watches: one look at
//! all running VMs' counters per round, whatever the number of watchers,
//! and none at all while nobody watches.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use zendiloos_kernel::RequestContext;

use super::Vms;
use crate::domain::{HostLimits, VmCounters, VmLoad, VmStatsSample};

/// How often the VMs' counters are looked at.
pub const STATS_EVERY: Duration = Duration::from_secs(3);
/// The machine's threads and memory are looked at again after this many
/// rounds.
const FACTS_EVERY: u32 = 20;

/// The samples, and whether the round runs.
pub(super) struct Hub {
    sender: watch::Sender<Option<VmStatsSample>>,
    watchers: Arc<AtomicUsize>,
    running: AtomicBool,
}

impl Default for Hub {
    fn default() -> Self {
        Self { sender: watch::channel(None).0, watchers: Arc::default(), running: AtomicBool::new(false) }
    }
}

/// One who watches; the rounds stop once nobody does.
struct Watching(Arc<AtomicUsize>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The VMs' load as it is looked at, for as long as this is held.
pub struct VmStatsSubscription {
    receiver: watch::Receiver<Option<VmStatsSample>>,
    _watching: Watching,
}

impl VmStatsSubscription {
    /// The newest sample, if there is one yet.
    pub fn latest(&self) -> Option<VmStatsSample> {
        self.receiver.borrow().clone()
    }

    /// Waits for the next sample.
    pub async fn next(&mut self) -> Option<VmStatsSample> {
        loop {
            self.receiver.changed().await.ok()?;
            if let Some(sample) = self.receiver.borrow_and_update().clone() {
                return Some(sample);
            }
        }
    }
}

impl Vms {
    /// Watches the VMs' load: the rounds begin with the first watcher.
    pub fn watch_stats(&self) -> VmStatsSubscription {
        self.stats.watchers.fetch_add(1, Ordering::SeqCst);
        let subscription = VmStatsSubscription {
            receiver: self.stats.sender.subscribe(),
            _watching: Watching(Arc::clone(&self.stats.watchers)),
        };
        if !self.stats.running.swap(true, Ordering::SeqCst) {
            let this = self.clone();
            tokio::spawn(async move { this.look_while_watched().await });
        }
        subscription
    }

    async fn look_while_watched(&self) {
        let hub = &self.stats;
        let context = RequestContext::default();
        let mut ticks = tokio::time::interval(STATS_EVERY);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut before: Option<(tokio::time::Instant, BTreeMap<String, VmCounters>)> = None;
        let mut limits: Option<HostLimits> = None;
        let mut rounds = 0u32;
        loop {
            ticks.tick().await;
            if hub.watchers.load(Ordering::SeqCst) == 0 {
                hub.running.store(false, Ordering::SeqCst);
                // Someone came between the look and the stop, and found the
                // rounds running: they go on.
                if hub.watchers.load(Ordering::SeqCst) > 0 && !hub.running.swap(true, Ordering::SeqCst) {
                    continue;
                }
                hub.sender.send_replace(None);
                return;
            }
            if limits.is_none() || rounds.is_multiple_of(FACTS_EVERY) {
                match self.ports.host.facts(&context).await {
                    Ok(facts) => limits = Some(facts.limits),
                    Err(error) => tracing::debug!(%error, "vms: cannot look at the machine for the load"),
                }
            }
            rounds = rounds.wrapping_add(1);
            let vms = match self.ports.repository.list().await {
                Ok(vms) => vms,
                Err(error) => {
                    tracing::warn!(%error, "vms: cannot list the virtual machines for their load");
                    continue;
                }
            };
            let counters = match self.ports.host.counters(&context).await {
                Ok(counters) => counters,
                Err(error) => {
                    tracing::warn!(%error, "vms: libvirt cannot say what the virtual machines use");
                    continue;
                }
            };
            let now = tokio::time::Instant::now();
            let seconds = before.as_ref().map_or(0.0, |(at, _)| now.duration_since(*at).as_secs_f64());
            let earlier = before.as_ref().map(|(_, counters)| counters);
            let limits = limits.clone().unwrap_or_default();
            let mut loads = Vec::new();
            for vm in &vms {
                let Some(now) = counters.get(&vm.id) else { continue };
                let then = earlier.and_then(|earlier| earlier.get(&vm.id));
                let load = VmLoad::of(&vm.id, vm.settings.cpus, vm.settings.memory_mib, then, now, seconds);
                loads.push((vm.settings.cpus, load));
            }
            let shares: Vec<(u16, &VmLoad)> = loads.iter().map(|(cpus, load)| (*cpus, load)).collect();
            let cpu_percent = VmStatsSample::host_cpu_percent(&shares, limits.cores);
            let sample = VmStatsSample {
                at: self.ports.clock.now(),
                running: loads.len(),
                total: vms.len(),
                cpu_percent,
                memory_assigned_bytes: loads.iter().map(|(_, load)| load.memory_bytes).sum(),
                host_memory_bytes: limits.memory_bytes,
                host_cores: limits.cores,
                vms: loads.into_iter().map(|(_, load)| load).collect(),
            };
            before = Some((now, counters));
            hub.sender.send_replace(Some(sample));
        }
    }

    /// Whether the rounds run: for tests.
    #[cfg(any(test, feature = "testing"))]
    pub fn stats_running(&self) -> bool {
        self.stats.running.load(Ordering::SeqCst)
    }
}
