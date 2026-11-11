//! The deliveries on channels that have not gone out yet, kept in the
//! database so that a restart of zendiloosd loses none.
//!
//! A notification raised goes into the queue once for each channel its
//! kind is routed to, and out from there at once. A delivery that fails
//! for a reason that may pass (the network or name resolution not up yet
//! after a boot or an update, a server that does not answer or answers
//! "later") is tried again after a pause that doubles from a minute to an
//! hour, [`TRIES`] times in all: for about twelve hours. Counting tries
//! rather than hours keeps a clock that is set only once the network is up
//! from giving a notification up before it was tried. One that cannot
//! succeed (a password or an address refused, a request a webhook turns
//! down) fails for good at once. Failed ones are kept a while, so that the
//! channel's page tells what never arrived.
//!
//! A channel's notifications go out in the order they were raised: while
//! the oldest waits, those behind it wait with it, and a new one for the
//! channel, or a test that went out, tries them all at once. A delivery
//! cut off by a restart is tried again, so one may arrive twice; none is
//! lost.

use std::time::Duration;

use async_trait::async_trait;
use zendiloos_kernel::{PortError, Timestamp};

use crate::Notification;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueState {
    /// Tried (again) at its time.
    Waiting,
    /// Not tried again: it cannot succeed, or its tries ran out.
    Failed,
}

impl QueueState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Waiting, Self::Failed].into_iter().find(|state| state.as_str() == value)
    }
}

/// One notification for one channel that has not gone out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedDelivery {
    pub id: i64,
    pub channel_id: i64,
    /// As it was raised.
    pub notification: Notification,
    pub state: QueueState,
    /// How often it was tried.
    pub attempts: u32,
    /// Waiting: when it is tried (again); failed: when it failed for good.
    pub at: Timestamp,
    /// Why the last try failed.
    pub error: Option<String>,
}

/// Why a message did not go out, and whether trying again may help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendFailure {
    /// In the words of the server or the library.
    pub reason: String,
    /// It may pass: no network or name resolution yet, no answer, a
    /// connection cut off, an SMTP 4xx, an HTTP 5xx.
    pub transient: bool,
}

impl SendFailure {
    pub fn transient(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), transient: true }
    }

    /// It cannot succeed as things are: credentials, an address or the
    /// request refused, settings that are not usable.
    pub fn permanent(reason: impl Into<String>) -> Self {
        Self { reason: reason.into(), transient: false }
    }
}

/// The queue's records.
#[async_trait]
pub trait DeliveryQueue: Send + Sync {
    /// Queues `notification` once for each of `channels`, due at `at`.
    async fn enqueue(
        &self,
        notification: &Notification,
        channels: &[i64],
        at: Timestamp,
    ) -> Result<(), PortError>;

    /// What waits and what failed for good, oldest first.
    async fn queued(&self) -> Result<Vec<QueuedDelivery>, PortError>;

    /// It went out: it leaves the queue.
    async fn dequeue(&self, id: i64) -> Result<(), PortError>;

    /// Its state, tries, time and error as `delivery` has them.
    async fn record(&self, delivery: &QueuedDelivery) -> Result<(), PortError>;

    /// Forgets the failed ones that failed before `before`, and those beyond
    /// each channel's `keep` most recent.
    async fn prune(&self, before: Timestamp, keep: u32) -> Result<(), PortError>;
}

/// How often a delivery is tried at most. With [`pause`] the last try is
/// about twelve hours after the first.
pub const TRIES: u32 = 18;

/// How long a failed delivery is shown, and how many per channel.
pub const FAILED_KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);
pub const FAILED_KEPT: u32 = 20;

/// The pause after the `attempts`-th failed try: a minute, doubling, at
/// most an hour.
pub fn pause(attempts: u32) -> Duration {
    let minutes = 1u64 << attempts.saturating_sub(1).min(6);
    Duration::from_secs((minutes * 60).min(3600))
}

/// `delivery` after a try at `now` failed with `failure`: waiting for its
/// next try, or failed for good when it cannot succeed or its tries ran out.
pub fn after_failure(delivery: &QueuedDelivery, failure: &SendFailure, now: Timestamp) -> QueuedDelivery {
    let attempts = delivery.attempts.saturating_add(1);
    let (state, at) = if failure.transient && attempts < TRIES {
        (QueueState::Waiting, now.saturating_add(pause(attempts)))
    } else {
        (QueueState::Failed, now)
    };
    QueuedDelivery { state, attempts, at, error: Some(failure.reason.clone()), ..delivery.clone() }
}

/// The waiting deliveries of one channel, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelQueue {
    pub channel_id: i64,
    pub waiting: Vec<QueuedDelivery>,
}

impl ChannelQueue {
    /// When the channel is tried next: when the earliest of its waiting
    /// deliveries is due (a new one is due at once).
    pub fn due_at(&self) -> Option<Timestamp> {
        self.waiting.iter().map(|delivery| delivery.at).min()
    }
}

/// The waiting deliveries of `listed`, by channel in the order of their
/// oldest, each channel's oldest first.
pub fn by_channel(listed: Vec<QueuedDelivery>) -> Vec<ChannelQueue> {
    let mut queues: Vec<ChannelQueue> = Vec::new();
    let mut waiting: Vec<QueuedDelivery> =
        listed.into_iter().filter(|delivery| delivery.state == QueueState::Waiting).collect();
    waiting.sort_by_key(|delivery| delivery.id);
    for delivery in waiting {
        match queues.iter_mut().find(|queue| queue.channel_id == delivery.channel_id) {
            Some(queue) => queue.waiting.push(delivery),
            None => queues.push(ChannelQueue { channel_id: delivery.channel_id, waiting: vec![delivery] }),
        }
    }
    queues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Level;

    fn queued(id: i64, channel_id: i64, at: i64) -> QueuedDelivery {
        QueuedDelivery {
            id,
            channel_id,
            notification: Notification {
                id: 7,
                at: Timestamp::from_unix_seconds(1000),
                level: Level::Warning,
                code: "pool_trouble".into(),
                params: Default::default(),
            },
            state: QueueState::Waiting,
            attempts: 0,
            at: Timestamp::from_unix_seconds(at),
            error: None,
        }
    }

    #[test]
    fn the_pause_doubles_from_a_minute_to_an_hour() {
        let minutes: Vec<u64> = (1..=9).map(|n| pause(n).as_secs() / 60).collect();
        assert_eq!(minutes, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(pause(0), Duration::from_secs(60), "never less than a minute");
        assert_eq!(pause(u32::MAX), Duration::from_secs(3600));
        let hours = (1..TRIES).map(|n| pause(n).as_secs()).sum::<u64>() as f64 / 3600.0;
        assert!((11.0..13.0).contains(&hours), "tried for about twelve hours: {hours}");
    }

    #[test]
    fn what_may_pass_waits_until_the_tries_run_out() {
        let now = Timestamp::from_unix_seconds(5000);
        let later = after_failure(&queued(1, 3, 5000), &SendFailure::transient("no network"), now);
        assert_eq!(
            (later.state, later.attempts, later.at, later.error.as_deref()),
            (QueueState::Waiting, 1, Timestamp::from_unix_seconds(5060), Some("no network"))
        );
        let third = after_failure(
            &QueuedDelivery { attempts: 2, ..later.clone() },
            &SendFailure::transient("x"),
            now,
        );
        assert_eq!(third.at, Timestamp::from_unix_seconds(5240));

        let refused = after_failure(&later, &SendFailure::permanent("535 authentication failed"), now);
        assert_eq!((refused.state, refused.attempts, refused.at), (QueueState::Failed, 2, now));

        let last = QueuedDelivery { attempts: TRIES - 1, ..later };
        let given_up = after_failure(&last, &SendFailure::transient("no network"), now);
        assert_eq!((given_up.state, given_up.attempts), (QueueState::Failed, TRIES));
    }

    #[test]
    fn waiting_deliveries_are_grouped_by_channel_oldest_first() {
        let failed = QueuedDelivery { state: QueueState::Failed, ..queued(2, 3, 10) };
        let queues = by_channel(vec![
            queued(5, 4, 300),
            queued(4, 3, 200),
            failed,
            queued(1, 3, 100),
            queued(6, 4, 50),
        ]);
        let ids: Vec<(i64, Vec<i64>)> =
            queues.iter().map(|q| (q.channel_id, q.waiting.iter().map(|d| d.id).collect())).collect();
        assert_eq!(ids, [(3, vec![1, 4]), (4, vec![5, 6])]);
        assert_eq!(queues[1].due_at(), Some(Timestamp::from_unix_seconds(50)), "a new one makes it due");
        assert_eq!(ChannelQueue { channel_id: 1, waiting: vec![] }.due_at(), None);
        for state in [QueueState::Waiting, QueueState::Failed] {
            assert_eq!(QueueState::parse(state.as_str()), Some(state));
        }
        assert_eq!(QueueState::parse("sent"), None);
    }
}
