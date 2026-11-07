//! Channels that carry notifications beyond the interface, so that a
//! failing disk is noticed without a browser open: email and webhooks
//! (chat services, Gotify, ntfy, home automation).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

use crate::message::{Language, Message, message};
use crate::{Level, Notification, Notifier};

/// How a mail server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// TLS from the start (usually port 465).
    Tls,
    /// Plain, then upgraded with STARTTLS (usually port 587); refused if
    /// the server cannot.
    StartTls,
    /// Unencrypted, for a relay in the own network; never with a password.
    None,
}

impl Security {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tls => "tls",
            Self::StartTls => "starttls",
            Self::None => "none",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Tls, Self::StartTls, Self::None].into_iter().find(|s| s.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub server: String,
    pub port: u16,
    pub security: Security,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
}

/// What a webhook receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookFormat {
    /// A JSON object with `title`, `message`, `text`, `content`, `level`,
    /// `code`, `params`, `host` and `at`: enough for most services.
    Json,
    /// The message as plain text, the title in a `Title` header (ntfy).
    Text,
}

impl WebhookFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Text => "text",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Json, Self::Text].into_iter().find(|f| f.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Webhook {
    pub url: String,
    pub format: WebhookFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Email(Email),
    Webhook(Webhook),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSettings {
    pub name: String,
    pub target: Target,
    /// The least serious notifications it carries: `Info` (all),
    /// `Warning` (warnings, errors and the good news that ends them) or
    /// `Error`.
    pub min_level: Level,
    pub language: Language,
}

/// The outcome of the last delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub at: Timestamp,
    /// What went wrong, in the words of the server or the library.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub id: i64,
    pub settings: ChannelSettings,
    pub last: Option<Delivery>,
}

/// The most channels; a few are plenty.
pub const MAX_CHANNELS: usize = 10;
/// The most recipients of one email channel.
pub const MAX_RECIPIENTS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidChannel {
    #[error("a name has 1 to 64 characters, without line breaks")]
    Name,
    #[error("not a host name or address of a mail server")]
    Server,
    #[error("the port is from 1 to 65535")]
    Port,
    #[error("not an email address: {0}")]
    Address(String),
    #[error("an email channel has 1 to {MAX_RECIPIENTS} recipients")]
    Recipients,
    #[error("a user name or password cannot contain line breaks")]
    Credentials,
    #[error("a password is only sent encrypted: choose TLS or STARTTLS")]
    PlainPassword,
    /// The stored password goes only to the server it was given for.
    #[error("type the password again when the server, port, encryption or user name changes")]
    RetypePassword,
    #[error("a webhook is an http:// or https:// address without spaces")]
    Url,
    #[error("the least serious level is info, warning or error")]
    Level,
}

fn is_address(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else { return false };
    (3..=254).contains(&value.len())
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains('@')
        && !value.chars().any(|c| c.is_whitespace() || c.is_control() || "<>,;\"()[]\\".contains(c))
}

fn is_host(value: &str) -> bool {
    (1..=253).contains(&value.len())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c))
        && !value.starts_with('-')
}

impl ChannelSettings {
    /// The settings, trimmed and checked.
    pub fn checked(mut self) -> Result<Self, InvalidChannel> {
        self.name = self.name.trim().to_owned();
        if !(1..=64).contains(&self.name.chars().count()) || self.name.chars().any(char::is_control) {
            return Err(InvalidChannel::Name);
        }
        if self.min_level == Level::Success {
            return Err(InvalidChannel::Level);
        }
        match &mut self.target {
            Target::Email(email) => {
                email.server = email.server.trim().to_owned();
                if !is_host(&email.server) {
                    return Err(InvalidChannel::Server);
                }
                if email.port == 0 {
                    return Err(InvalidChannel::Port);
                }
                email.from = email.from.trim().to_owned();
                for address in std::iter::once(&mut email.from).chain(email.to.iter_mut()) {
                    *address = address.trim().to_owned();
                    if !is_address(address) {
                        return Err(InvalidChannel::Address(address.clone()));
                    }
                }
                if email.to.is_empty() || email.to.len() > MAX_RECIPIENTS {
                    return Err(InvalidChannel::Recipients);
                }
                email.username = email.username.take().map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
                email.password = email.password.take().filter(|p| !p.is_empty());
                let credentials = email.username.iter().chain(email.password.iter());
                if credentials.clone().any(|value| value.len() > 1024 || value.chars().any(char::is_control))
                {
                    return Err(InvalidChannel::Credentials);
                }
                if email.security == Security::None && email.password.is_some() {
                    return Err(InvalidChannel::PlainPassword);
                }
            }
            Target::Webhook(webhook) => {
                webhook.url = webhook.url.trim().to_owned();
                let rest =
                    webhook.url.strip_prefix("https://").or_else(|| webhook.url.strip_prefix("http://"));
                let host = rest.and_then(|rest| rest.split(['/', '?', '#']).next()).unwrap_or_default();
                let bad = |c: char| c.is_whitespace() || c.is_control();
                if host.is_empty() || webhook.url.len() > 2048 || webhook.url.chars().any(bad) {
                    return Err(InvalidChannel::Url);
                }
            }
        }
        Ok(self)
    }

    /// Whether a notification of `level` goes out on this channel.
    pub fn carries(&self, level: Level) -> bool {
        match self.min_level {
            Level::Info => true,
            Level::Success | Level::Warning => level != Level::Info,
            Level::Error => level == Level::Error,
        }
    }
}

/// How an update treats the stored password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordChange {
    Keep,
    /// The one in the new settings, or none.
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Done,
    NotFound,
    NameTaken,
}

#[async_trait]
pub trait ChannelRepository: Send + Sync {
    /// By name.
    async fn list(&self) -> Result<Vec<Channel>, PortError>;

    /// `None` if the name is taken.
    async fn create(&self, settings: &ChannelSettings, at: Timestamp) -> Result<Option<Channel>, PortError>;

    async fn update(&self, id: i64, settings: &ChannelSettings, at: Timestamp) -> Result<Saved, PortError>;

    /// `false` if there is no such channel.
    async fn delete(&self, id: i64) -> Result<bool, PortError>;

    async fn delivered(&self, id: i64, delivery: &Delivery) -> Result<(), PortError>;
}

/// Delivers messages; adapters for SMTP and HTTP.
#[async_trait]
pub trait ChannelSender: Send + Sync {
    /// Delivers one message; the error in words an administrator can act on.
    async fn send(
        &self,
        target: &Target,
        message: &Message,
        notification: &Notification,
        host: &str,
    ) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("there is no such channel")]
    NotFound,
    #[error("the name is taken")]
    NameTaken,
    #[error("at most {MAX_CHANNELS} channels")]
    TooMany,
    #[error(transparent)]
    Invalid(#[from] InvalidChannel),
    /// The test message did not go out.
    #[error("{0}")]
    Delivery(String),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// A delivery that takes longer than this has failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Managing the channels and delivering notifications on them.
#[derive(Clone)]
pub struct Channels {
    repository: Arc<dyn ChannelRepository>,
    sender: Arc<dyn ChannelSender>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
    /// The NAS's name, in every message.
    host: String,
}

impl Channels {
    pub fn new(
        repository: Arc<dyn ChannelRepository>,
        sender: Arc<dyn ChannelSender>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        host: String,
    ) -> Self {
        Self { repository, sender, audit, clock, host }
    }

    pub async fn list(&self) -> Result<Vec<Channel>, ChannelError> {
        Ok(self.repository.list().await?)
    }

    async fn find(&self, id: i64) -> Result<Channel, ChannelError> {
        self.repository.list().await?.into_iter().find(|c| c.id == id).ok_or(ChannelError::NotFound)
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry = AuditEntry::new(self.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    pub async fn create(
        &self,
        settings: ChannelSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Channel, ChannelError> {
        let settings = settings.checked()?;
        if self.repository.list().await?.len() >= MAX_CHANNELS {
            return Err(ChannelError::TooMany);
        }
        let created =
            self.repository.create(&settings, self.clock.now()).await?.ok_or(ChannelError::NameTaken)?;
        self.audit("notify.channel_create", Outcome::Success, &settings.name, actor, request).await;
        Ok(created)
    }

    pub async fn update(
        &self,
        id: i64,
        settings: ChannelSettings,
        password: PasswordChange,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Channel, ChannelError> {
        let mut settings = settings.checked()?;
        let existing = self.find(id).await?;
        if let (PasswordChange::Keep, Target::Email(new), Target::Email(old)) =
            (&password, &mut settings.target, &existing.settings.target)
        {
            let elsewhere = (&new.server, new.port, new.security, &new.username)
                != (&old.server, old.port, old.security, &old.username);
            if old.password.is_some() && elsewhere {
                return Err(InvalidChannel::RetypePassword.into());
            }
            new.password.clone_from(&old.password);
            // Checked again: keeping a password must not send it in clear.
            settings = settings.checked()?;
        }
        match self.repository.update(id, &settings, self.clock.now()).await? {
            Saved::Done => {}
            Saved::NotFound => return Err(ChannelError::NotFound),
            Saved::NameTaken => return Err(ChannelError::NameTaken),
        }
        self.audit("notify.channel_update", Outcome::Success, &settings.name, actor, request).await;
        self.find(id).await
    }

    pub async fn delete(&self, id: i64, actor: &Actor, request: &RequestContext) -> Result<(), ChannelError> {
        let existing = self.find(id).await?;
        if !self.repository.delete(id).await? {
            return Err(ChannelError::NotFound);
        }
        self.audit("notify.channel_delete", Outcome::Success, &existing.settings.name, actor, request).await;
        Ok(())
    }

    /// Sends a test message now; its error, if any, is the answer.
    pub async fn test(&self, id: i64, actor: &Actor, request: &RequestContext) -> Result<(), ChannelError> {
        let channel = self.find(id).await?;
        let notification = Notification {
            id: 0,
            at: self.clock.now(),
            level: Level::Info,
            code: "channel_test".to_owned(),
            params: Default::default(),
        };
        let delivery = self.send(&channel, &notification).await;
        let outcome = if delivery.error.is_none() { Outcome::Success } else { Outcome::Failure };
        self.audit("notify.channel_test", outcome, &channel.settings.name, actor, request).await;
        delivery.error.map_or(Ok(()), |error| Err(ChannelError::Delivery(error)))
    }

    async fn send(&self, channel: &Channel, notification: &Notification) -> Delivery {
        let text = message(notification, &self.host, channel.settings.language);
        let sent = tokio::time::timeout(
            SEND_TIMEOUT,
            self.sender.send(&channel.settings.target, &text, notification, &self.host),
        )
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {} seconds", SEND_TIMEOUT.as_secs())));
        let delivery = Delivery { at: self.clock.now(), error: sent.err() };
        if let Err(error) = self.repository.delivered(channel.id, &delivery).await {
            tracing::warn!(%error, "channels: cannot record a delivery");
        }
        delivery
    }

    /// Delivers `notification` on every channel that carries its level.
    pub async fn deliver(&self, notification: &Notification) {
        let channels = match self.repository.list().await {
            Ok(channels) => channels,
            Err(error) => return tracing::warn!(%error, "channels: cannot read them"),
        };
        for channel in channels.iter().filter(|c| c.settings.carries(notification.level)) {
            let delivery = self.send(channel, notification).await;
            match delivery.error {
                None => tracing::info!(
                    channel = channel.settings.name,
                    code = notification.code,
                    "channels: delivered"
                ),
                Some(error) => {
                    tracing::warn!(
                        channel = channel.settings.name,
                        code = notification.code,
                        error,
                        "channels: not delivered"
                    );
                }
            }
        }
    }

    /// Delivers every notification `notifier` publishes from now on, each
    /// on a task of its own so a slow server holds up no other.
    pub fn spawn(self, notifier: &Notifier) -> JoinHandle<()> {
        let mut events = notifier.subscribe();
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(notification) => {
                        let channels = self.clone();
                        tokio::spawn(async move { channels.deliver(&notification).await });
                    }
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "channels: notifications came too fast, some were not sent");
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::testing::{MemoryChannels, RecordingSender};

    fn email(to: &str) -> ChannelSettings {
        ChannelSettings {
            name: " Admins ".into(),
            target: Target::Email(Email {
                server: "mail.example.org".into(),
                port: 587,
                security: Security::StartTls,
                username: Some("nas".into()),
                password: Some("secret".into()),
                from: "nas@example.org".into(),
                to: vec![to.into()],
            }),
            min_level: Level::Warning,
            language: Language::German,
        }
    }

    fn webhook(url: &str) -> ChannelSettings {
        ChannelSettings {
            name: "Chat".into(),
            target: Target::Webhook(Webhook { url: url.into(), format: WebhookFormat::Json }),
            min_level: Level::Error,
            language: Language::English,
        }
    }

    #[test]
    fn settings_are_checked() {
        let checked = email("me@example.org").checked().unwrap();
        assert_eq!(checked.name, "Admins");
        let with = |change: fn(&mut Email)| {
            let mut settings = email("me@example.org");
            if let Target::Email(e) = &mut settings.target {
                change(e);
            }
            settings.checked()
        };
        assert_eq!(with(|e| e.to = vec![]), Err(InvalidChannel::Recipients));
        assert_eq!(with(|e| e.to = vec!["me".into()]), Err(InvalidChannel::Address("me".into())));
        assert_eq!(with(|e| e.from = "a@b@c.d".into()), Err(InvalidChannel::Address("a@b@c.d".into())));
        assert_eq!(with(|e| e.server = "mail example".into()), Err(InvalidChannel::Server));
        assert_eq!(with(|e| e.port = 0), Err(InvalidChannel::Port));
        assert_eq!(with(|e| e.password = Some("a\nb".into())), Err(InvalidChannel::Credentials));
        assert_eq!(with(|e| e.security = Security::None), Err(InvalidChannel::PlainPassword));
        assert!(
            with(|e| (e.security, e.password) = (Security::None, None)).is_ok(),
            "a relay without password"
        );
        assert!(webhook("https://chat.example.org/hooks/x?token=1").checked().is_ok());
        for url in ["ftp://x", "https://", "https://a b", "chat.example.org"] {
            assert_eq!(webhook(url).checked(), Err(InvalidChannel::Url), "{url}");
        }
    }

    #[test]
    fn securities_and_formats_are_named_and_read_back() {
        for security in [Security::Tls, Security::StartTls, Security::None] {
            assert_eq!(Security::parse(security.as_str()), Some(security));
        }
        for format in [WebhookFormat::Json, WebhookFormat::Text] {
            assert_eq!(WebhookFormat::parse(format.as_str()), Some(format));
        }
        assert_eq!((Security::parse("ssl"), WebhookFormat::parse("xml")), (None, None));
        for level in [Level::Info, Level::Success, Level::Warning, Level::Error] {
            assert_eq!(Level::parse(level.as_str()), Some(level));
        }
        assert_eq!(Level::parse("fatal"), None);
        let mut odd = webhook("https://x.example");
        odd.name = "  ".into();
        assert_eq!(odd.checked(), Err(InvalidChannel::Name));
        let mut good_news = webhook("https://x.example");
        good_news.min_level = Level::Success;
        assert_eq!(good_news.checked(), Err(InvalidChannel::Level), "success is no level to start from");
    }

    #[tokio::test]
    async fn notifications_published_go_out_by_themselves() {
        let sender = Arc::new(RecordingSender::default());
        let channels = Channels::new(
            Arc::new(MemoryChannels::default()),
            sender.clone(),
            Arc::new(Audit::default()),
            Arc::new(Fixed),
            "nas".into(),
        );
        channels.create(email("me@example.org"), &alice(), &RequestContext::default()).await.unwrap();
        let notifier =
            crate::Notifier::new(Arc::new(crate::testing::MemoryNotifications::default()), Arc::new(Fixed));
        let delivering = channels.clone().spawn(&notifier);
        notifier
            .notify(crate::NewNotification::new(Level::Error, "snapshot_failed").with("share", "photos"))
            .await;
        for _ in 0..100 {
            if !sender.sent.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(sender.sent.lock().unwrap()[0].1.title, "[nas] Kein Snapshot von photos");
        delivering.abort();
        let missing = channels.test(99, &alice(), &RequestContext::default()).await;
        assert!(matches!(missing, Err(ChannelError::NotFound)), "{missing:?}");
    }

    #[tokio::test]
    async fn there_is_a_limit_of_channels() {
        let channels = Channels::new(
            Arc::new(MemoryChannels::default()),
            Arc::new(RecordingSender::default()),
            Arc::new(Audit::default()),
            Arc::new(Fixed),
            "nas".into(),
        );
        let request = RequestContext::default();
        for n in 0..MAX_CHANNELS {
            channels
                .create(
                    webhook(&format!("https://x.example/{n}")).named(&format!("Chat {n}")),
                    &alice(),
                    &request,
                )
                .await
                .unwrap();
        }
        let one_more = channels.create(webhook("https://x.example").named("More"), &alice(), &request).await;
        assert!(matches!(one_more, Err(ChannelError::TooMany)), "{one_more:?}");
        let missing =
            channels.update(99, webhook("https://x.example"), PasswordChange::Keep, &alice(), &request).await;
        assert!(matches!(missing, Err(ChannelError::NotFound)), "{missing:?}");
    }

    impl ChannelSettings {
        fn named(mut self, name: &str) -> Self {
            self.name = name.to_owned();
            self
        }
    }

    #[test]
    fn a_channel_carries_its_level_and_more_serious_ones() {
        let warnings = email("me@example.org");
        assert!(!warnings.carries(Level::Info));
        assert!(warnings.carries(Level::Success), "the good news that ends a warning");
        assert!(warnings.carries(Level::Error));
        let errors = webhook("https://x.example");
        assert!(!errors.carries(Level::Warning) && errors.carries(Level::Error));
    }

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(1_793_836_800)
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<&'static str>>);

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.0.lock().unwrap().push(entry.action);
            Ok(())
        }
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    #[tokio::test]
    async fn notifications_go_out_on_the_channels_that_carry_them() {
        let sender = Arc::new(RecordingSender::default());
        let audit = Arc::new(Audit::default());
        let channels = Channels::new(
            Arc::new(MemoryChannels::default()),
            sender.clone(),
            audit.clone(),
            Arc::new(Fixed),
            "nas".into(),
        );
        let request = RequestContext::default();
        let mail = channels.create(email("me@example.org"), &alice(), &request).await.unwrap();
        let chat = channels.create(webhook("https://broken.example"), &alice(), &request).await.unwrap();
        let taken = channels.create(email("other@example.org"), &alice(), &request).await;
        assert!(matches!(taken, Err(ChannelError::NameTaken)));

        let degraded = Notification {
            id: 7,
            at: Fixed.now(),
            level: Level::Warning,
            code: "pool_trouble".into(),
            params: [("pool".into(), "tank".into()), ("state".into(), "degraded".into())].into(),
        };
        channels.deliver(&degraded).await;
        let sent = sender.sent.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "the chat only carries errors");
        assert_eq!(sent[0].1.title, "[nas] Pool tank ist beeinträchtigt");

        let failed = channels.test(chat.id, &alice(), &request).await;
        assert!(
            matches!(failed, Err(ChannelError::Delivery(ref e)) if e == "connection refused"),
            "{failed:?}"
        );
        let listed = channels.list().await.unwrap();
        let chat = listed.iter().find(|c| c.id == chat.id).unwrap();
        assert_eq!(chat.last.as_ref().and_then(|d| d.error.as_deref()), Some("connection refused"));
        assert_eq!(listed.iter().find(|c| c.id == mail.id).unwrap().last.as_ref().unwrap().error, None);

        // The stored password goes nowhere else: not unencrypted, not to
        // another server.
        let mut plain = email("me@example.org");
        if let Target::Email(e) = &mut plain.target {
            (e.security, e.password) = (Security::None, None);
        }
        let kept = channels.update(mail.id, plain.clone(), PasswordChange::Keep, &alice(), &request).await;
        assert!(matches!(kept, Err(ChannelError::Invalid(InvalidChannel::RetypePassword))), "{kept:?}");
        let mut moved = email("me@example.org");
        if let Target::Email(e) = &mut moved.target {
            e.server = "smtp.elsewhere.example".into();
        }
        let redirected = channels.update(mail.id, moved, PasswordChange::Keep, &alice(), &request).await;
        assert!(
            matches!(redirected, Err(ChannelError::Invalid(InvalidChannel::RetypePassword))),
            "{redirected:?}"
        );
        channels.update(mail.id, plain, PasswordChange::Replace, &alice(), &request).await.unwrap();
        channels.delete(mail.id, &alice(), &request).await.unwrap();
        assert!(matches!(channels.delete(mail.id, &alice(), &request).await, Err(ChannelError::NotFound)));
        assert_eq!(
            *audit.0.lock().unwrap(),
            [
                "notify.channel_create",
                "notify.channel_create",
                "notify.channel_test",
                "notify.channel_update",
                "notify.channel_delete"
            ]
        );
    }
}
