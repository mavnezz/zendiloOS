use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zendiloos_kernel::Clock;
use zendiloos_system::application::ports::{LiveMetrics, MetricsSubscription};
use zendiloos_system::domain::{HISTORY_SAMPLES, MetricsSample};

use super::metrics::HostMetrics;

/// [`LiveMetrics`] fed by one sampling task.
#[derive(Clone)]
pub struct MetricsHub {
    receiver: watch::Receiver<Option<MetricsSample>>,
    /// The last [`HISTORY_SAMPLES`], oldest first.
    recent: Arc<Mutex<VecDeque<MetricsSample>>>,
}

impl MetricsHub {
    /// Samples the host every `interval` until the returned task is aborted.
    pub fn spawn(interval: Duration, clock: Arc<dyn Clock>) -> (Self, JoinHandle<()>) {
        let (sender, receiver) = watch::channel(None);
        let recent = Arc::new(Mutex::new(VecDeque::with_capacity(HISTORY_SAMPLES)));
        let kept = Arc::clone(&recent);
        let task = tokio::spawn(async move {
            let mut metrics = match HostMetrics::new() {
                Ok(metrics) => Some(metrics),
                Err(error) => {
                    tracing::warn!(%error, "cannot read metrics; the dashboard stays empty");
                    return;
                }
            };
            let mut ticks = tokio::time::interval(interval);
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticks.tick().await; // the first tick is immediate; CPU usage needs a gap
            loop {
                ticks.tick().await;
                let mut taken = metrics.take().expect("returned after every sample");
                let at = clock.now();
                // Reads files under /proc and /sys; keep it off the async workers.
                let Ok((back, sample)) = tokio::task::spawn_blocking(move || {
                    let sample = taken.sample(at);
                    (taken, sample)
                })
                .await
                else {
                    tracing::warn!("metrics sampling failed; stopping");
                    return;
                };
                metrics = Some(back);
                {
                    let mut kept = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if kept.len() == HISTORY_SAMPLES {
                        kept.pop_front();
                    }
                    kept.push_back(sample.clone());
                }
                sender.send_replace(Some(sample));
            }
        });
        (Self { receiver, recent }, task)
    }

    /// A hub that never produces a sample, for tests.
    pub fn idle() -> Self {
        Self { receiver: watch::channel(None).1, recent: Arc::default() }
    }
}

impl LiveMetrics for MetricsHub {
    fn latest(&self) -> Option<MetricsSample> {
        self.receiver.borrow().clone()
    }

    fn recent(&self) -> Vec<MetricsSample> {
        self.recent.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().cloned().collect()
    }

    fn subscribe(&self) -> Box<dyn MetricsSubscription> {
        let mut receiver = self.receiver.clone();
        receiver.mark_unchanged();
        Box::new(Subscription(receiver))
    }
}

struct Subscription(watch::Receiver<Option<MetricsSample>>);

#[async_trait]
impl MetricsSubscription for Subscription {
    async fn next(&mut self) -> Option<MetricsSample> {
        loop {
            self.0.changed().await.ok()?;
            if let Some(sample) = self.0.borrow_and_update().clone() {
                return Some(sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SystemClock;

    #[tokio::test]
    async fn subscribers_receive_new_samples() {
        let (hub, task) = MetricsHub::spawn(Duration::from_millis(250), Arc::new(SystemClock));
        let mut subscription = hub.subscribe();

        let first = tokio::time::timeout(Duration::from_secs(5), subscription.next()).await.unwrap().unwrap();
        let second =
            tokio::time::timeout(Duration::from_secs(5), subscription.next()).await.unwrap().unwrap();

        assert!(first.memory.total_bytes > 0);
        assert!(second.at >= first.at);
        assert_eq!(hub.latest().map(|s| s.at), Some(second.at));
        task.abort();
    }
}
