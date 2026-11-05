//! Notifications: things an administrator should know about, such as a
//! degraded pool or a failing disk. Stored, so that a browser opened later
//! sees them, and published to the browsers that are open (WebSocket topic
//! `notifications`).
//!
//! A notification carries a stable code and parameters, not text: the
//! interface words it in the user's language.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use zendiloos_kernel::{Clock, PortError, Timestamp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Success => "success",
            Level::Warning => "warning",
            Level::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "info" => Level::Info,
            "success" => Level::Success,
            "warning" => Level::Warning,
            "error" => Level::Error,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotification {
    pub level: Level,
    /// Stable, e.g. `pool_degraded`.
    pub code: String,
    /// E.g. `{"pool": "tank"}`.
    pub params: BTreeMap<String, String>,
}

impl NewNotification {
    pub fn new(level: Level, code: &str) -> Self {
        Self { level, code: code.to_owned(), params: BTreeMap::new() }
    }

    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.params.insert(key.to_owned(), value.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: i64,
    pub at: Timestamp,
    pub level: Level,
    pub code: String,
    pub params: BTreeMap<String, String>,
}

#[async_trait]
pub trait NotificationRepository: Send + Sync {
    async fn create(&self, at: Timestamp, notification: NewNotification) -> Result<Notification, PortError>;

    /// The most recent ones, newest first.
    async fn recent(&self, limit: u32) -> Result<Vec<Notification>, PortError>;
}

/// Stores and publishes notifications. Cheap to clone.
#[derive(Clone)]
pub struct Notifier {
    repository: Arc<dyn NotificationRepository>,
    clock: Arc<dyn Clock>,
    events: broadcast::Sender<Notification>,
}

impl Notifier {
    pub fn new(repository: Arc<dyn NotificationRepository>, clock: Arc<dyn Clock>) -> Self {
        let (events, _) = broadcast::channel(64);
        Self { repository, clock, events }
    }

    /// Stores and publishes; a storage failure is logged, since whoever
    /// notifies cannot do anything about it.
    pub async fn notify(&self, notification: NewNotification) {
        tracing::info!(code = notification.code, params = ?notification.params, "notification");
        match self.repository.create(self.clock.now(), notification).await {
            Ok(stored) => {
                let _ = self.events.send(stored);
            }
            Err(error) => tracing::error!(%error, "cannot store a notification"),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    pub async fn recent(&self, limit: u32) -> Result<Vec<Notification>, PortError> {
        self.repository.recent(limit.clamp(1, 200)).await
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct MemoryNotifications(Mutex<Vec<Notification>>);

    impl MemoryNotifications {
        pub fn all(&self) -> Vec<Notification> {
            self.0.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl NotificationRepository for MemoryNotifications {
        async fn create(&self, at: Timestamp, new: NewNotification) -> Result<Notification, PortError> {
            let mut all = self.0.lock().unwrap();
            let stored = Notification {
                id: i64::try_from(all.len()).unwrap() + 1,
                at,
                level: new.level,
                code: new.code,
                params: new.params,
            };
            all.push(stored.clone());
            Ok(stored)
        }

        async fn recent(&self, limit: u32) -> Result<Vec<Notification>, PortError> {
            Ok(self.0.lock().unwrap().iter().rev().take(limit as usize).cloned().collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::MemoryNotifications;
    use super::*;

    struct Fixed;

    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(42)
        }
    }

    #[tokio::test]
    async fn stores_and_publishes() {
        let repository = Arc::new(MemoryNotifications::default());
        let notifier = Notifier::new(repository.clone(), Arc::new(Fixed));
        let mut events = notifier.subscribe();

        notifier.notify(NewNotification::new(Level::Warning, "pool_degraded").with("pool", "tank")).await;

        let published = events.recv().await.unwrap();
        assert_eq!((published.code.as_str(), published.params["pool"].as_str()), ("pool_degraded", "tank"));
        assert_eq!(repository.all(), std::slice::from_ref(&published));
        assert_eq!(notifier.recent(10).await.unwrap(), [published]);
    }
}
