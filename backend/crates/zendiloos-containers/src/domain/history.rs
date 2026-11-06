//! How much each container used over the last hours, sample by sample.

use std::collections::VecDeque;

use zendiloos_kernel::Timestamp;

use super::engine::UsageSample;

/// One container at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct UsagePoint {
    pub container: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    /// Since the sample before; 0 for the first or after a restart.
    pub received_per_second: f64,
    pub sent_per_second: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Moment {
    pub at: Timestamp,
    pub points: Vec<UsagePoint>,
}

/// The newest samples, at most `capacity`; containers by name, which
/// stays when a container is made anew.
#[derive(Debug)]
pub struct UsageHistory {
    capacity: usize,
    moments: VecDeque<Moment>,
    last: Option<UsageSample>,
}

impl UsageHistory {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), moments: VecDeque::new(), last: None }
    }

    pub fn record(&mut self, sample: UsageSample) {
        let seconds = self
            .last
            .as_ref()
            .map(|last| sample.at.duration_since(last.at).as_secs_f64())
            .filter(|seconds| *seconds > 0.0);
        let points = sample
            .containers
            .iter()
            .map(|usage| {
                let before =
                    self.last.as_ref().and_then(|last| last.containers.iter().find(|c| c.id == usage.id));
                // A counter that went down belongs to a restarted container.
                let rate = |now: u64, then: Option<u64>| match (then, seconds) {
                    (Some(then), Some(seconds)) if now >= then => (now - then) as f64 / seconds,
                    _ => 0.0,
                };
                UsagePoint {
                    container: usage.name.clone(),
                    cpu_percent: usage.cpu_percent,
                    memory_bytes: usage.memory_bytes,
                    received_per_second: rate(usage.received_bytes, before.map(|b| b.received_bytes)),
                    sent_per_second: rate(usage.sent_bytes, before.map(|b| b.sent_bytes)),
                }
            })
            .collect();
        self.moments.push_back(Moment { at: sample.at, points });
        while self.moments.len() > self.capacity {
            self.moments.pop_front();
        }
        self.last = Some(sample);
    }

    /// The moments from `since` on, oldest first, with only `containers`
    /// if any are named.
    pub fn since(&self, since: Timestamp, containers: &[String]) -> Vec<Moment> {
        self.moments
            .iter()
            .filter(|moment| moment.at >= since)
            .map(|moment| Moment {
                at: moment.at,
                points: moment
                    .points
                    .iter()
                    .filter(|point| containers.is_empty() || containers.contains(&point.container))
                    .cloned()
                    .collect(),
            })
            .collect()
    }

    /// The newest sample as Docker gave it.
    pub fn latest(&self) -> Option<&UsageSample> {
        self.last.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::engine::ContainerUsage;

    fn sample(at: i64, received: u64) -> UsageSample {
        UsageSample {
            at: Timestamp::from_unix_seconds(at),
            containers: vec![ContainerUsage {
                id: "abc".into(),
                name: "teslamate".into(),
                cpu_percent: 1.5,
                memory_bytes: 70,
                memory_limit_bytes: 1000,
                received_bytes: received,
                sent_bytes: 0,
            }],
            processors: 4,
        }
    }

    #[test]
    fn rates_come_from_the_counters_between_samples() {
        let mut history = UsageHistory::new(10);
        history.record(sample(0, 1_000));
        history.record(sample(20, 5_000));
        history.record(sample(40, 100));

        let moments = history.since(Timestamp::from_unix_seconds(0), &[]);
        let rates: Vec<f64> = moments.iter().map(|m| m.points[0].received_per_second).collect();
        assert_eq!(rates, [0.0, 200.0, 0.0], "none at first, none after a restart");
        assert_eq!(history.latest().unwrap().at, Timestamp::from_unix_seconds(40));
    }

    #[test]
    fn only_the_newest_are_kept_and_asked_for() {
        let mut history = UsageHistory::new(2);
        for at in [0, 20, 40] {
            history.record(sample(at, 0));
        }
        let at: Vec<i64> =
            history.since(Timestamp::from_unix_seconds(0), &[]).iter().map(|m| m.at.unix_seconds()).collect();
        assert_eq!(at, [20, 40]);
        assert!(history.since(Timestamp::from_unix_seconds(30), &["other".into()])[0].points.is_empty());
    }
}
