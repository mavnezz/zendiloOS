use std::sync::Arc;

use crate::application::ports::{LiveMetrics, MetricsSubscription};
use crate::domain::{MetricsHistory, MetricsSample};

/// Query: the live metrics, as the latest sample and a stream of new ones.
#[derive(Clone)]
pub struct WatchMetrics {
    live: Arc<dyn LiveMetrics>,
}

impl WatchMetrics {
    pub fn new(live: Arc<dyn LiveMetrics>) -> Self {
        Self { live }
    }

    pub fn latest(&self) -> Option<MetricsSample> {
        self.live.latest()
    }

    pub fn subscribe(&self) -> Box<dyn MetricsSubscription> {
        self.live.subscribe()
    }

    /// The last minutes, for charts that should not start empty.
    pub fn history(&self) -> MetricsHistory {
        MetricsHistory::of(&self.live.recent())
    }
}
