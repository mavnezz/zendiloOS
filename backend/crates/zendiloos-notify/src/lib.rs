//! Notifications: things an administrator should know about, such as a
//! degraded pool or a failing disk. Stored, so that a browser opened later
//! sees them, and published to the browsers that are open (WebSocket topic
//! `notifications`).
//!
//! A notification carries a stable code and parameters, not text: the
//! interface words it in the user's language.
//!
//! Where a notification goes is routed by its kind ([`routes`]): kept in
//! the notification center (stored and published to the browsers) or not,
//! and on which channels ([`channels`]), which get every notification
//! raised, kept or not.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use zendiloos_kernel::{Clock, PortError, Timestamp};

pub mod channels;
pub mod message;
pub mod routes;

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

/// How many notifications are kept; the oldest go when new ones arrive.
pub const KEPT: u32 = 500;

/// How far one account has dealt with the notifications: everything up to
/// `read_up_to` counts as read, everything up to `cleared_up_to` is no
/// longer listed for it. IDs only grow, so a mark stays valid while old
/// notifications are deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Marks {
    pub read_up_to: i64,
    pub cleared_up_to: i64,
}

#[async_trait]
pub trait NotificationRepository: Send + Sync {
    async fn create(&self, at: Timestamp, notification: NewNotification) -> Result<Notification, PortError>;

    /// The most recent ones with an ID above `after`, newest first.
    async fn recent(&self, after: i64, limit: u32) -> Result<Vec<Notification>, PortError>;

    /// Deletes all but the `keep` most recent ones.
    async fn prune(&self, keep: u32) -> Result<(), PortError>;

    /// The account's marks; zero for an account that never set any.
    async fn marks(&self, account_id: i64) -> Result<Marks, PortError>;

    /// Raises the account's marks to at least `to`. A mark never goes back,
    /// so two browsers cannot undo each other.
    async fn advance_marks(&self, account_id: i64, to: Marks) -> Result<(), PortError>;
}

/// Stores and publishes notifications. Cheap to clone.
#[derive(Clone)]
pub struct Notifier {
    repository: Arc<dyn NotificationRepository>,
    clock: Arc<dyn Clock>,
    /// Which kinds are kept; without, every one is.
    routes: Option<Arc<dyn routes::RouteRepository>>,
    /// Those kept, for the browsers.
    events: broadcast::Sender<Notification>,
    /// Every one raised, kept or not, for the channels.
    outgoing: broadcast::Sender<Notification>,
}

impl Notifier {
    pub fn new(repository: Arc<dyn NotificationRepository>, clock: Arc<dyn Clock>) -> Self {
        let (events, _) = broadcast::channel(64);
        let (outgoing, _) = broadcast::channel(64);
        Self { repository, clock, routes: None, events, outgoing }
    }

    /// Keeps only the kinds `routes` keep in zendiloOS.
    #[must_use]
    pub fn routed(mut self, routes: Arc<dyn routes::RouteRepository>) -> Self {
        self.routes = Some(routes);
        self
    }

    /// Whether a notification is kept in the notification center; when the
    /// routes cannot be read, it is.
    async fn kept(&self, notification: &NewNotification) -> bool {
        let Some(routes) = &self.routes else { return true };
        let kind = routes::kind_of(&notification.code, notification.level);
        match routes.routes().await {
            Ok(routes) => routes.in_app(kind),
            Err(error) => {
                tracing::warn!(%error, "cannot read where notifications go; kept");
                true
            }
        }
    }

    /// Stores and publishes it if its kind is kept in zendiloOS (keeping
    /// only the [`KEPT`] most recent ones), and hands it to the channels
    /// either way, even when it cannot be stored. A storage failure is
    /// logged, since whoever notifies cannot do anything about it.
    pub async fn notify(&self, notification: NewNotification) {
        tracing::info!(code = notification.code, params = ?notification.params, "notification");
        let unstored = |NewNotification { level, code, params }| Notification {
            id: 0,
            at: self.clock.now(),
            level,
            code,
            params,
        };
        if !self.kept(&notification).await {
            let _ = self.outgoing.send(unstored(notification));
            return;
        }
        match self.repository.create(self.clock.now(), notification.clone()).await {
            Ok(stored) => {
                let _ = self.events.send(stored.clone());
                let _ = self.outgoing.send(stored);
            }
            Err(error) => {
                tracing::error!(%error, "cannot store a notification");
                let _ = self.outgoing.send(unstored(notification));
                return;
            }
        }
        if let Err(error) = self.repository.prune(KEPT).await {
            tracing::warn!(%error, "cannot delete old notifications");
        }
    }

    /// The notifications kept, as they come: for the browsers.
    pub fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.events.subscribe()
    }

    /// Every notification raised, kept or not, as it comes: for the
    /// channels.
    pub fn outgoing(&self) -> broadcast::Receiver<Notification> {
        self.outgoing.subscribe()
    }

    /// The most recent notifications the account has not cleared away,
    /// newest first, and how far it has read.
    pub async fn for_account(
        &self,
        account_id: i64,
        limit: u32,
    ) -> Result<(Vec<Notification>, Marks), PortError> {
        let marks = self.repository.marks(account_id).await?;
        let recent = self.repository.recent(marks.cleared_up_to, limit.clamp(1, 200)).await?;
        Ok((recent, marks))
    }

    /// Everything up to `up_to` counts as read for the account.
    pub async fn mark_read(&self, account_id: i64, up_to: i64) -> Result<(), PortError> {
        let up_to = self.existing(up_to).await?;
        self.repository.advance_marks(account_id, Marks { read_up_to: up_to, cleared_up_to: 0 }).await
    }

    /// Everything up to `up_to` is read and no longer listed for the account.
    pub async fn clear(&self, account_id: i64, up_to: i64) -> Result<(), PortError> {
        let up_to = self.existing(up_to).await?;
        self.repository.advance_marks(account_id, Marks { read_up_to: up_to, cleared_up_to: up_to }).await
    }

    /// `up_to`, but no further than the newest notification: a mark ahead of
    /// it would hide notifications that have not happened yet.
    async fn existing(&self, up_to: i64) -> Result<i64, PortError> {
        let newest = self.repository.recent(0, 1).await?.first().map_or(0, |n| n.id);
        Ok(up_to.clamp(0, newest))
    }
}

#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub struct MemoryNotifications {
        all: Mutex<Vec<Notification>>,
        last_id: Mutex<i64>,
        marks: Mutex<HashMap<i64, Marks>>,
    }

    impl MemoryNotifications {
        pub fn all(&self) -> Vec<Notification> {
            self.all.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl NotificationRepository for MemoryNotifications {
        async fn create(&self, at: Timestamp, new: NewNotification) -> Result<Notification, PortError> {
            let mut last_id = self.last_id.lock().unwrap();
            *last_id += 1;
            let stored =
                Notification { id: *last_id, at, level: new.level, code: new.code, params: new.params };
            self.all.lock().unwrap().push(stored.clone());
            Ok(stored)
        }

        async fn recent(&self, after: i64, limit: u32) -> Result<Vec<Notification>, PortError> {
            let all = self.all.lock().unwrap();
            Ok(all.iter().rev().filter(|n| n.id > after).take(limit as usize).cloned().collect())
        }

        async fn prune(&self, keep: u32) -> Result<(), PortError> {
            let mut all = self.all.lock().unwrap();
            let excess = all.len().saturating_sub(keep as usize);
            all.drain(..excess);
            Ok(())
        }

        async fn marks(&self, account_id: i64) -> Result<Marks, PortError> {
            Ok(self.marks.lock().unwrap().get(&account_id).copied().unwrap_or_default())
        }

        async fn advance_marks(&self, account_id: i64, to: Marks) -> Result<(), PortError> {
            let mut marks = self.marks.lock().unwrap();
            let current = marks.entry(account_id).or_default();
            current.read_up_to = current.read_up_to.max(to.read_up_to);
            current.cleared_up_to = current.cleared_up_to.max(to.cleared_up_to);
            Ok(())
        }
    }

    /// [`ChannelRepository`](crate::channels::ChannelRepository) in memory.
    #[derive(Default)]
    pub struct MemoryChannels {
        channels: Mutex<Vec<crate::channels::Channel>>,
        server: Mutex<Option<crate::channels::MailServer>>,
    }

    /// [`RouteRepository`](crate::routes::RouteRepository) in memory.
    #[derive(Default)]
    pub struct MemoryRoutes(pub Mutex<crate::routes::Routes>);

    #[async_trait]
    impl crate::routes::RouteRepository for MemoryRoutes {
        async fn routes(&self) -> Result<crate::routes::Routes, PortError> {
            Ok(self.0.lock().unwrap().clone())
        }

        async fn replace(&self, routes: &crate::routes::Routes) -> Result<(), PortError> {
            *self.0.lock().unwrap() = routes.clone();
            Ok(())
        }
    }

    /// [`AccountAddresses`](crate::channels::AccountAddresses) in memory:
    /// account ID → address, `None` for an account without one, `"-"` for
    /// a disabled one; accounts not listed are gone.
    #[derive(Default)]
    pub struct MemoryAddresses(pub Mutex<HashMap<i64, Option<String>>>);

    #[async_trait]
    impl crate::channels::AccountAddresses for MemoryAddresses {
        async fn address(&self, account_id: i64) -> Result<crate::channels::AccountAddress, PortError> {
            use crate::channels::AccountAddress;
            Ok(match self.0.lock().unwrap().get(&account_id) {
                Some(Some(address)) if address == "-" => AccountAddress::Disabled,
                Some(Some(address)) => AccountAddress::Address(address.clone()),
                Some(None) => AccountAddress::Missing,
                None => AccountAddress::Gone,
            })
        }
    }

    #[async_trait]
    impl crate::channels::ChannelRepository for MemoryChannels {
        async fn list(&self) -> Result<Vec<crate::channels::Channel>, PortError> {
            let mut all = self.channels.lock().unwrap().clone();
            all.sort_by(|a, b| a.settings.name.cmp(&b.settings.name));
            Ok(all)
        }

        async fn create(
            &self,
            settings: &crate::channels::ChannelSettings,
            _: Timestamp,
        ) -> Result<Option<crate::channels::Channel>, PortError> {
            let mut all = self.channels.lock().unwrap();
            if all.iter().any(|c| c.settings.name == settings.name) {
                return Ok(None);
            }
            let id = all.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            let channel = crate::channels::Channel { id, settings: settings.clone(), last: None };
            all.push(channel.clone());
            Ok(Some(channel))
        }

        async fn update(
            &self,
            id: i64,
            settings: &crate::channels::ChannelSettings,
            _: Timestamp,
        ) -> Result<crate::channels::Saved, PortError> {
            let mut all = self.channels.lock().unwrap();
            if all.iter().any(|c| c.id != id && c.settings.name == settings.name) {
                return Ok(crate::channels::Saved::NameTaken);
            }
            let Some(channel) = all.iter_mut().find(|c| c.id == id) else {
                return Ok(crate::channels::Saved::NotFound);
            };
            channel.settings = settings.clone();
            Ok(crate::channels::Saved::Done)
        }

        async fn delete(&self, id: i64) -> Result<bool, PortError> {
            let mut all = self.channels.lock().unwrap();
            let before = all.len();
            all.retain(|c| c.id != id);
            Ok(all.len() < before)
        }

        async fn delivered(&self, id: i64, delivery: &crate::channels::Delivery) -> Result<(), PortError> {
            if let Some(channel) = self.channels.lock().unwrap().iter_mut().find(|c| c.id == id) {
                channel.last = Some(delivery.clone());
            }
            Ok(())
        }

        async fn mail_server(&self) -> Result<Option<crate::channels::MailServer>, PortError> {
            Ok(self.server.lock().unwrap().clone())
        }

        async fn set_mail_server(
            &self,
            server: &crate::channels::MailServer,
            _: Timestamp,
        ) -> Result<(), PortError> {
            *self.server.lock().unwrap() = Some(server.clone());
            Ok(())
        }
    }

    /// [`ChannelSender`](crate::channels::ChannelSender) that keeps what it
    /// is given; a channel whose name starts with `broken` fails.
    #[derive(Default)]
    pub struct RecordingSender {
        pub sent: Mutex<Vec<(String, crate::message::Message)>>,
    }

    #[async_trait]
    impl crate::channels::ChannelSender for RecordingSender {
        async fn send(
            &self,
            target: &crate::channels::Target,
            message: &crate::message::Message,
            _: &Notification,
            _: &str,
        ) -> Result<(), String> {
            let to = match target {
                crate::channels::Target::Email(email) => email.to.join(","),
                crate::channels::Target::Webhook(webhook) => webhook.url.clone(),
            };
            if to.contains("broken") {
                return Err("connection refused".into());
            }
            self.sent.lock().unwrap().push((to, message.clone()));
            Ok(())
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
        assert_eq!(notifier.for_account(1, 10).await.unwrap(), (vec![published], Marks::default()));
    }

    /// A database that takes nothing in.
    struct Broken;

    #[async_trait]
    impl NotificationRepository for Broken {
        async fn create(&self, _: Timestamp, _: NewNotification) -> Result<Notification, PortError> {
            Err(PortError::new("notifications", "disk full"))
        }
        async fn recent(&self, _: i64, _: u32) -> Result<Vec<Notification>, PortError> {
            Ok(Vec::new())
        }
        async fn prune(&self, _: u32) -> Result<(), PortError> {
            Ok(())
        }
        async fn marks(&self, _: i64) -> Result<Marks, PortError> {
            Ok(Marks::default())
        }
        async fn advance_marks(&self, _: i64, _: Marks) -> Result<(), PortError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn goes_out_even_when_it_cannot_be_stored() {
        let notifier = Notifier::new(Arc::new(Broken), Arc::new(Fixed));
        let (mut events, mut outgoing) = (notifier.subscribe(), notifier.outgoing());

        notifier.notify(NewNotification::new(Level::Error, "pool_trouble").with("pool", "tank")).await;

        let sent = outgoing.try_recv().unwrap();
        assert_eq!(
            (sent.id, sent.code.as_str(), sent.at),
            (0, "pool_trouble", Timestamp::from_unix_seconds(42))
        );
        assert!(events.try_recv().is_err(), "nothing stored, nothing to show");
    }

    #[tokio::test]
    async fn keeps_only_the_most_recent() {
        let repository = Arc::new(MemoryNotifications::default());
        let notifier = Notifier::new(repository.clone(), Arc::new(Fixed));

        for _ in 0..KEPT + 3 {
            notifier.notify(NewNotification::new(Level::Info, "scrub_finished")).await;
        }

        let kept = repository.all();
        assert_eq!(kept.len(), KEPT as usize);
        assert_eq!(kept.first().unwrap().id, 4);
    }

    #[tokio::test]
    async fn each_account_reads_and_clears_on_its_own() {
        let notifier = Notifier::new(Arc::new(MemoryNotifications::default()), Arc::new(Fixed));
        for code in ["pool_trouble", "pool_recovered", "scrub_finished"] {
            notifier.notify(NewNotification::new(Level::Info, code)).await;
        }
        let (alice, bob) = (1, 2);

        notifier.mark_read(alice, 2).await.unwrap();
        notifier.clear(bob, 1).await.unwrap();

        let (listed, marks) = notifier.for_account(alice, 10).await.unwrap();
        assert_eq!(listed.iter().map(|n| n.id).collect::<Vec<_>>(), [3, 2, 1]);
        assert_eq!(marks, Marks { read_up_to: 2, cleared_up_to: 0 });
        let (listed, marks) = notifier.for_account(bob, 10).await.unwrap();
        assert_eq!(listed.iter().map(|n| n.id).collect::<Vec<_>>(), [3, 2]);
        assert_eq!(marks, Marks { read_up_to: 1, cleared_up_to: 1 });
    }

    #[tokio::test]
    async fn marks_never_go_back_or_ahead_of_the_newest() {
        let notifier = Notifier::new(Arc::new(MemoryNotifications::default()), Arc::new(Fixed));
        notifier.notify(NewNotification::new(Level::Info, "scrub_finished")).await;
        notifier.notify(NewNotification::new(Level::Info, "scrub_finished")).await;

        notifier.mark_read(1, 2).await.unwrap();
        notifier.mark_read(1, 1).await.unwrap();
        assert_eq!(notifier.for_account(1, 10).await.unwrap().1.read_up_to, 2);

        // A mark far ahead would hide what comes next.
        notifier.clear(1, i64::MAX).await.unwrap();
        notifier.notify(NewNotification::new(Level::Warning, "pool_trouble")).await;
        let (listed, marks) = notifier.for_account(1, 10).await.unwrap();
        assert_eq!(listed.iter().map(|n| n.code.as_str()).collect::<Vec<_>>(), ["pool_trouble"]);
        assert_eq!(marks, Marks { read_up_to: 2, cleared_up_to: 2 });
    }
}
