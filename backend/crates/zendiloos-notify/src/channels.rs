//! Channels that carry notifications beyond the interface, so that a
//! failing disk is noticed without a browser open: email and webhooks
//! (chat services, Gotify, ntfy, home automation).
//!
//! Mail goes out through one mail server, set once; an email channel names
//! only its recipients: zendiloOS accounts (at the address the account has
//! when a mail goes out) and other addresses. Which notifications a
//! channel carries is the routing table's business ([`crate::routes`]).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

use crate::message::{Language, Message, message};
use crate::routes::{KINDS, Kind, RouteRepository, Routes, find, kind_of};
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

/// The mail server every email channel sends through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailServer {
    pub server: String,
    pub port: u16,
    pub security: Security,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The sender's address.
    pub from: String,
}

/// One mail as it goes out: the server and the addresses it goes to.
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

/// Where one message is delivered: what [`ChannelSender`] is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Email(Email),
    Webhook(Webhook),
}

/// Someone an email channel writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recipient {
    /// A zendiloOS account, at the address it has when a mail goes out.
    Account(i64),
    Address(String),
}

/// Where a channel delivers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Through the mail server, to these recipients.
    Email {
        recipients: Vec<Recipient>,
    },
    Webhook(Webhook),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSettings {
    pub name: String,
    pub destination: Destination,
    /// What the channel carries of kinds nobody chose for it yet (see
    /// [`crate::routes`]): `Info` (all), `Warning` (warnings, errors and the
    /// good news that ends them) or `Error`. A new channel starts with
    /// `Warning`; once it was the channel's only choice.
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

/// The most channels.
pub const MAX_CHANNELS: usize = 20;
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
    #[error("an email channel has 1 to {MAX_RECIPIENTS} recipients, each once")]
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
    #[error("there is no kind of notification called {0}")]
    Kind(String),
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

fn checked_address(address: &mut String) -> Result<(), InvalidChannel> {
    *address = address.trim().to_owned();
    if is_address(address) { Ok(()) } else { Err(InvalidChannel::Address(address.clone())) }
}

impl MailServer {
    /// The settings, trimmed and checked.
    pub fn checked(mut self) -> Result<Self, InvalidChannel> {
        self.server = self.server.trim().to_owned();
        if !is_host(&self.server) {
            return Err(InvalidChannel::Server);
        }
        if self.port == 0 {
            return Err(InvalidChannel::Port);
        }
        checked_address(&mut self.from)?;
        self.username = self.username.take().map(|u| u.trim().to_owned()).filter(|u| !u.is_empty());
        self.password = self.password.take().filter(|p| !p.is_empty());
        let credentials = self.username.iter().chain(self.password.iter());
        if credentials.clone().any(|value| value.len() > 1024 || value.chars().any(char::is_control)) {
            return Err(InvalidChannel::Credentials);
        }
        if self.security == Security::None && self.password.is_some() {
            return Err(InvalidChannel::PlainPassword);
        }
        Ok(self)
    }

    /// A mail to `to` through this server.
    pub fn mail(&self, to: Vec<String>) -> Email {
        Email {
            server: self.server.clone(),
            port: self.port,
            security: self.security,
            username: self.username.clone(),
            password: self.password.clone(),
            from: self.from.clone(),
            to,
        }
    }
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
        match &mut self.destination {
            Destination::Email { recipients } => {
                for recipient in recipients.iter_mut() {
                    match recipient {
                        Recipient::Address(address) => checked_address(address)?,
                        Recipient::Account(id) if *id <= 0 => return Err(InvalidChannel::Recipients),
                        Recipient::Account(_) => {}
                    }
                }
                let mut unique = recipients.clone();
                unique.sort_by_key(|r| format!("{r:?}").to_lowercase());
                unique.dedup_by_key(|r| format!("{r:?}").to_lowercase());
                if recipients.is_empty()
                    || recipients.len() > MAX_RECIPIENTS
                    || unique.len() != recipients.len()
                {
                    return Err(InvalidChannel::Recipients);
                }
            }
            Destination::Webhook(webhook) => {
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

    /// `false` if there is no such channel. Its routes go with it.
    async fn delete(&self, id: i64) -> Result<bool, PortError>;

    async fn delivered(&self, id: i64, delivery: &Delivery) -> Result<(), PortError>;

    /// `None` until one is set up.
    async fn mail_server(&self) -> Result<Option<MailServer>, PortError>;

    async fn set_mail_server(&self, server: &MailServer, at: Timestamp) -> Result<(), PortError>;
}

/// What an account's address is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountAddress {
    Address(String),
    /// The account has no email address.
    Missing,
    /// The account is disabled: nothing goes to it.
    Disabled,
    /// There is no such account any more.
    Gone,
}

/// The email addresses of zendiloOS accounts.
#[async_trait]
pub trait AccountAddresses: Send + Sync {
    async fn address(&self, account_id: i64) -> Result<AccountAddress, PortError>;
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

/// A recipient of an email channel and the address mail goes to now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecipientState {
    pub recipient: Recipient,
    pub address: AccountAddress,
}

/// Where one kind of notification goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindRoute {
    pub kind: Kind,
    pub in_app: bool,
    /// The IDs of the channels it goes out on.
    pub channels: Vec<i64>,
}

/// A choice for one kind: kept in zendiloOS, and the channels it goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindChoice {
    pub kind: String,
    pub in_app: bool,
    pub channels: Vec<i64>,
}

/// A delivery that takes longer than this has failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// What channels need from outside.
#[derive(Clone)]
pub struct ChannelPorts {
    pub repository: Arc<dyn ChannelRepository>,
    pub routes: Arc<dyn RouteRepository>,
    pub addresses: Arc<dyn AccountAddresses>,
    pub sender: Arc<dyn ChannelSender>,
    pub audit: Arc<dyn AuditLog>,
    pub clock: Arc<dyn Clock>,
}

/// Managing the channels, the mail server and the routes, and delivering
/// notifications on the channels their kind is routed to.
#[derive(Clone)]
pub struct Channels {
    ports: ChannelPorts,
    /// The NAS's name, in every message.
    host: String,
}

impl Channels {
    pub fn new(ports: ChannelPorts, host: String) -> Self {
        Self { ports, host }
    }

    pub async fn list(&self) -> Result<Vec<Channel>, ChannelError> {
        Ok(self.ports.repository.list().await?)
    }

    async fn find(&self, id: i64) -> Result<Channel, ChannelError> {
        self.list().await?.into_iter().find(|c| c.id == id).ok_or(ChannelError::NotFound)
    }

    async fn audit(
        &self,
        action: &'static str,
        outcome: Outcome,
        target: &str,
        actor: &Actor,
        request: &RequestContext,
    ) {
        let entry =
            AuditEntry::new(self.ports.clock.now(), action, outcome, request).by(actor).target(target);
        if let Err(error) = self.ports.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }

    /// The recipients of an email channel and where mail to them goes now;
    /// none for a webhook.
    pub async fn recipients(&self, channel: &Channel) -> Result<Vec<RecipientState>, ChannelError> {
        let Destination::Email { recipients } = &channel.settings.destination else { return Ok(Vec::new()) };
        let mut states = Vec::with_capacity(recipients.len());
        for recipient in recipients {
            let address = match recipient {
                Recipient::Address(address) => AccountAddress::Address(address.clone()),
                Recipient::Account(id) => self.ports.addresses.address(*id).await?,
            };
            states.push(RecipientState { recipient: recipient.clone(), address });
        }
        Ok(states)
    }

    pub async fn create(
        &self,
        settings: ChannelSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Channel, ChannelError> {
        let settings = settings.checked()?;
        let repository = &self.ports.repository;
        if repository.list().await?.len() >= MAX_CHANNELS {
            return Err(ChannelError::TooMany);
        }
        let created =
            repository.create(&settings, self.ports.clock.now()).await?.ok_or(ChannelError::NameTaken)?;
        self.audit("notify.channel_create", Outcome::Success, &settings.name, actor, request).await;
        Ok(created)
    }

    /// Changes a channel's name, destination and language; its baseline
    /// level stays.
    pub async fn update(
        &self,
        id: i64,
        settings: ChannelSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Channel, ChannelError> {
        let existing = self.find(id).await?;
        let settings = ChannelSettings { min_level: existing.settings.min_level, ..settings }.checked()?;
        match self.ports.repository.update(id, &settings, self.ports.clock.now()).await? {
            Saved::Done => {}
            Saved::NotFound => return Err(ChannelError::NotFound),
            Saved::NameTaken => return Err(ChannelError::NameTaken),
        }
        self.audit("notify.channel_update", Outcome::Success, &settings.name, actor, request).await;
        self.find(id).await
    }

    pub async fn delete(&self, id: i64, actor: &Actor, request: &RequestContext) -> Result<(), ChannelError> {
        let existing = self.find(id).await?;
        if !self.ports.repository.delete(id).await? {
            return Err(ChannelError::NotFound);
        }
        self.audit("notify.channel_delete", Outcome::Success, &existing.settings.name, actor, request).await;
        Ok(())
    }

    /// Sends a test message on this channel alone, now; its error, if any,
    /// is the answer.
    pub async fn test(&self, id: i64, actor: &Actor, request: &RequestContext) -> Result<(), ChannelError> {
        let channel = self.find(id).await?;
        let notification = Notification {
            id: 0,
            at: self.ports.clock.now(),
            level: Level::Info,
            code: "channel_test".to_owned(),
            params: Default::default(),
        };
        let delivery = self.send(&channel, &notification).await;
        let outcome = if delivery.error.is_none() { Outcome::Success } else { Outcome::Failure };
        self.audit("notify.channel_test", outcome, &channel.settings.name, actor, request).await;
        delivery.error.map_or(Ok(()), |error| Err(ChannelError::Delivery(error)))
    }

    pub async fn mail_server(&self) -> Result<Option<MailServer>, ChannelError> {
        Ok(self.ports.repository.mail_server().await?)
    }

    /// Sets up the mail server. With [`PasswordChange::Keep`] the stored
    /// password stays, but only for the same server, port, encryption and
    /// user name: it goes nowhere else.
    pub async fn set_mail_server(
        &self,
        server: MailServer,
        password: PasswordChange,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<MailServer, ChannelError> {
        let mut server = server.checked()?;
        if password == PasswordChange::Keep
            && let Some(old) = self.ports.repository.mail_server().await?
        {
            let elsewhere = (&server.server, server.port, server.security, &server.username)
                != (&old.server, old.port, old.security, &old.username);
            if old.password.is_some() && elsewhere {
                return Err(InvalidChannel::RetypePassword.into());
            }
            server.password = old.password;
            // Checked again: keeping a password must not send it in clear.
            server = server.checked()?;
        }
        self.ports.repository.set_mail_server(&server, self.ports.clock.now()).await?;
        self.audit("notify.mail_server_update", Outcome::Success, &server.server, actor, request).await;
        Ok(server)
    }

    /// Where every kind of notification goes now, in the table's order.
    pub async fn routes(&self) -> Result<Vec<KindRoute>, ChannelError> {
        let routes = self.ports.routes.routes().await?;
        let channels = self.list().await?;
        Ok(KINDS
            .iter()
            .map(|kind| KindRoute {
                kind: *kind,
                in_app: routes.in_app(kind.key),
                channels: channels
                    .iter()
                    .filter(|c| routes.to_channel(c.id, c.settings.min_level, kind.key, kind.level))
                    .map(|c| c.id)
                    .collect(),
            })
            .collect())
    }

    /// Replaces the routes of the kinds given; the others keep theirs.
    pub async fn set_routes(
        &self,
        choices: Vec<KindChoice>,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Vec<KindRoute>, ChannelError> {
        let channels = self.list().await?;
        let mut routes = self.ports.routes.routes().await?;
        for choice in &choices {
            if find(&choice.kind).is_none() {
                return Err(InvalidChannel::Kind(choice.kind.clone()).into());
            }
            if let Some(unknown) = choice.channels.iter().find(|id| !channels.iter().any(|c| c.id == **id)) {
                tracing::debug!(channel = unknown, "routes: no such channel");
                return Err(ChannelError::NotFound);
            }
            routes.in_app.insert(choice.kind.clone(), choice.in_app);
            for channel in &channels {
                routes
                    .channels
                    .insert((channel.id, choice.kind.clone()), choice.channels.contains(&channel.id));
            }
        }
        // What belongs to channels that are gone no longer counts.
        routes.channels.retain(|(id, _), _| channels.iter().any(|c| c.id == *id));
        self.ports.routes.replace(&routes).await?;
        self.audit("notify.routes_update", Outcome::Success, "notifications", actor, request).await;
        self.routes().await
    }

    /// What `channel` delivers to now, or why it cannot deliver.
    async fn target(&self, channel: &Channel) -> Result<Target, String> {
        match &channel.settings.destination {
            Destination::Webhook(webhook) => Ok(Target::Webhook(webhook.clone())),
            Destination::Email { .. } => {
                let server = self.ports.repository.mail_server().await.map_err(|e| e.to_string())?;
                let server = server.ok_or("no mail server is set up")?;
                let states = self.recipients(channel).await.map_err(|e| e.to_string())?;
                let to: Vec<String> = states
                    .into_iter()
                    .filter_map(|state| match state.address {
                        AccountAddress::Address(address) => Some(address),
                        AccountAddress::Missing | AccountAddress::Disabled | AccountAddress::Gone => None,
                    })
                    .collect();
                if to.is_empty() {
                    return Err("none of the recipients has an email address".to_owned());
                }
                Ok(Target::Email(server.mail(to)))
            }
        }
    }

    async fn send(&self, channel: &Channel, notification: &Notification) -> Delivery {
        let text = message(notification, &self.host, channel.settings.language);
        let sent = match self.target(channel).await {
            Ok(target) => tokio::time::timeout(
                SEND_TIMEOUT,
                self.ports.sender.send(&target, &text, notification, &self.host),
            )
            .await
            .unwrap_or_else(|_| Err(format!("no answer within {} seconds", SEND_TIMEOUT.as_secs()))),
            Err(error) => Err(error),
        };
        let delivery = Delivery { at: self.ports.clock.now(), error: sent.err() };
        if let Err(error) = self.ports.repository.delivered(channel.id, &delivery).await {
            tracing::warn!(%error, "channels: cannot record a delivery");
        }
        delivery
    }

    /// Delivers `notification` on every channel its kind is routed to.
    pub async fn deliver(&self, notification: &Notification) {
        let (channels, routes) = match (self.ports.repository.list().await, self.ports.routes.routes().await)
        {
            (Ok(channels), Ok(routes)) => (channels, routes),
            (Err(error), _) | (_, Err(error)) => {
                return tracing::warn!(%error, "channels: cannot read them or their routes");
            }
        };
        let kind = kind_of(&notification.code, notification.level);
        for channel in channels.iter().filter(|c| carried(&routes, c, kind, notification.level)) {
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

    /// Delivers every notification `notifier` raises from now on, kept in
    /// zendiloOS or not, each on a task of its own so a slow server holds
    /// up no other.
    pub fn spawn(self, notifier: &Notifier) -> JoinHandle<()> {
        let mut events = notifier.outgoing();
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

fn carried(routes: &Routes, channel: &Channel, kind: &str, level: Level) -> bool {
    routes.to_channel(channel.id, channel.settings.min_level, kind, level)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::routes::Routes;
    use crate::testing::{
        MemoryAddresses, MemoryChannels, MemoryNotifications, MemoryRoutes, RecordingSender,
    };

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

    fn request() -> RequestContext {
        RequestContext::default()
    }

    struct Setup {
        channels: Channels,
        sender: Arc<RecordingSender>,
        audit: Arc<Audit>,
        routes: Arc<MemoryRoutes>,
        addresses: Arc<MemoryAddresses>,
    }

    fn setup() -> Setup {
        let sender = Arc::new(RecordingSender::default());
        let audit = Arc::new(Audit::default());
        let routes = Arc::new(MemoryRoutes::default());
        let addresses = Arc::new(MemoryAddresses::default());
        let channels = Channels::new(
            ChannelPorts {
                repository: Arc::new(MemoryChannels::default()),
                routes: routes.clone(),
                addresses: addresses.clone(),
                sender: sender.clone(),
                audit: audit.clone(),
                clock: Arc::new(Fixed),
            },
            "nas".into(),
        );
        Setup { channels, sender, audit, routes, addresses }
    }

    fn server() -> MailServer {
        MailServer {
            server: " mail.example.org ".into(),
            port: 587,
            security: Security::StartTls,
            username: Some("nas".into()),
            password: Some("secret".into()),
            from: "nas@example.org".into(),
        }
    }

    fn email(name: &str, recipients: Vec<Recipient>) -> ChannelSettings {
        ChannelSettings {
            name: name.into(),
            destination: Destination::Email { recipients },
            min_level: Level::Warning,
            language: Language::German,
        }
    }

    fn webhook(name: &str, url: &str) -> ChannelSettings {
        ChannelSettings {
            name: name.into(),
            destination: Destination::Webhook(Webhook { url: url.into(), format: WebhookFormat::Json }),
            min_level: Level::Error,
            language: Language::English,
        }
    }

    fn address(value: &str) -> Recipient {
        Recipient::Address(value.into())
    }

    fn degraded(level: Level) -> Notification {
        Notification {
            id: 7,
            at: Fixed.now(),
            level,
            code: "pool_trouble".into(),
            params: [("pool".into(), "tank".into()), ("state".into(), "degraded".into())].into(),
        }
    }

    fn sent_to(sender: &RecordingSender) -> Vec<String> {
        sender.sent.lock().unwrap().iter().map(|(to, _)| to.clone()).collect()
    }

    #[test]
    fn settings_and_the_mail_server_are_checked() {
        let checked = email(" Admins ", vec![address(" me@example.org ")]).checked().unwrap();
        assert_eq!(checked.name, "Admins");
        assert_eq!(checked.destination, Destination::Email { recipients: vec![address("me@example.org")] });
        let bad = |recipients| email("Admins", recipients).checked();
        assert_eq!(bad(vec![]), Err(InvalidChannel::Recipients));
        assert_eq!(bad(vec![address("me")]), Err(InvalidChannel::Address("me".into())));
        assert_eq!(bad(vec![Recipient::Account(0)]), Err(InvalidChannel::Recipients));
        assert_eq!(bad(vec![Recipient::Account(3), Recipient::Account(3)]), Err(InvalidChannel::Recipients));
        assert_eq!(bad(vec![address("a@b.de"), address("A@B.de")]), Err(InvalidChannel::Recipients));
        assert_eq!(
            bad((0..11).map(|n| address(&format!("u{n}@b.de"))).collect()),
            Err(InvalidChannel::Recipients)
        );
        assert!(webhook("Chat", "https://chat.example.org/hooks/x?token=1").checked().is_ok());
        for url in ["ftp://x", "https://", "https://a b", "chat.example.org"] {
            assert_eq!(webhook("Chat", url).checked(), Err(InvalidChannel::Url), "{url}");
        }
        assert_eq!(webhook("  ", "https://x.example").checked(), Err(InvalidChannel::Name));
        let mut good_news = webhook("Chat", "https://x.example");
        good_news.min_level = Level::Success;
        assert_eq!(good_news.checked(), Err(InvalidChannel::Level), "success is no level to start from");

        assert_eq!(server().checked().unwrap().server, "mail.example.org");
        let with = |change: fn(&mut MailServer)| {
            let mut settings = server();
            change(&mut settings);
            settings.checked()
        };
        assert_eq!(with(|s| s.from = "a@b@c.d".into()), Err(InvalidChannel::Address("a@b@c.d".into())));
        assert_eq!(with(|s| s.server = "mail example".into()), Err(InvalidChannel::Server));
        assert_eq!(with(|s| s.port = 0), Err(InvalidChannel::Port));
        assert_eq!(with(|s| s.password = Some("a\nb".into())), Err(InvalidChannel::Credentials));
        assert_eq!(with(|s| s.security = Security::None), Err(InvalidChannel::PlainPassword));
        assert!(
            with(|s| (s.security, s.password) = (Security::None, None)).is_ok(),
            "a relay without password"
        );
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
    }

    #[tokio::test]
    async fn mail_goes_through_the_one_server_to_each_recipient_as_they_are_now() {
        let t = setup();
        let mail = email(
            "Family",
            vec![
                Recipient::Account(2),
                Recipient::Account(3),
                Recipient::Account(4),
                Recipient::Account(5),
                address("bob@example.org"),
            ],
        );
        let family = t.channels.create(mail, &alice(), &request()).await.unwrap();
        t.addresses.0.lock().unwrap().extend([
            (2, Some("anna@example.org".to_owned())),
            (3, None),
            (5, Some("-".to_owned())),
        ]);

        let none = t.channels.test(family.id, &alice(), &request()).await;
        assert!(
            matches!(none, Err(ChannelError::Delivery(ref e)) if e == "no mail server is set up"),
            "{none:?}"
        );

        t.channels.set_mail_server(server(), PasswordChange::Replace, &alice(), &request()).await.unwrap();
        t.channels.test(family.id, &alice(), &request()).await.unwrap();
        assert_eq!(sent_to(&t.sender), ["anna@example.org,bob@example.org"]);
        let states = t.channels.recipients(&family).await.unwrap();
        assert_eq!(
            states.iter().map(|s| s.address.clone()).collect::<Vec<_>>(),
            [
                AccountAddress::Address("anna@example.org".into()),
                AccountAddress::Missing,
                AccountAddress::Gone,
                AccountAddress::Disabled,
                AccountAddress::Address("bob@example.org".into()),
            ]
        );

        // An account's new address is used from the next mail on.
        t.addresses.0.lock().unwrap().insert(2, Some("anna@new.example".into()));
        t.channels.deliver(&degraded(Level::Warning)).await;
        assert_eq!(sent_to(&t.sender)[1], "anna@new.example,bob@example.org");

        let lonely = t
            .channels
            .create(email("Nobody", vec![Recipient::Account(3)]), &alice(), &request())
            .await
            .unwrap();
        let failed = t.channels.test(lonely.id, &alice(), &request()).await;
        assert!(
            matches!(failed, Err(ChannelError::Delivery(ref e)) if e.contains("none of the recipients")),
            "{failed:?}"
        );
        let hook = Channel { id: 9, settings: webhook("Chat", "https://x.example"), last: None };
        assert!(t.channels.recipients(&hook).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_mail_password_goes_nowhere_else() {
        let t = setup();
        t.channels.set_mail_server(server(), PasswordChange::Replace, &alice(), &request()).await.unwrap();
        let mut plain = server();
        (plain.security, plain.password) = (Security::None, None);
        let kept =
            t.channels.set_mail_server(plain.clone(), PasswordChange::Keep, &alice(), &request()).await;
        assert!(matches!(kept, Err(ChannelError::Invalid(InvalidChannel::RetypePassword))), "{kept:?}");
        let mut moved = server();
        moved.server = "smtp.elsewhere.example".into();
        let redirected = t.channels.set_mail_server(moved, PasswordChange::Keep, &alice(), &request()).await;
        assert!(
            matches!(redirected, Err(ChannelError::Invalid(InvalidChannel::RetypePassword))),
            "{redirected:?}"
        );
        let mut new_sender = server();
        (new_sender.from, new_sender.password) = ("alerts@example.org".into(), None);
        let saved = t.channels.set_mail_server(new_sender, PasswordChange::Keep, &alice(), &request()).await;
        assert_eq!(saved.unwrap().password.as_deref(), Some("secret"), "kept for the same server");
        t.channels.set_mail_server(plain, PasswordChange::Replace, &alice(), &request()).await.unwrap();
        assert_eq!(t.channels.mail_server().await.unwrap().unwrap().password, None);
        assert_eq!(*t.audit.0.lock().unwrap(), ["notify.mail_server_update"; 3]);
    }

    #[tokio::test]
    async fn notifications_go_where_their_kind_is_routed() {
        let t = setup();
        t.channels.set_mail_server(server(), PasswordChange::Replace, &alice(), &request()).await.unwrap();
        let mail = t
            .channels
            .create(email("Admins", vec![address("me@example.org")]), &alice(), &request())
            .await
            .unwrap();
        let chat =
            t.channels.create(webhook("Chat", "https://chat.example"), &alice(), &request()).await.unwrap();

        // By default: by each channel's level and the kind's usual level.
        t.channels.deliver(&degraded(Level::Warning)).await;
        assert_eq!(sent_to(&t.sender), ["me@example.org"], "the chat carries errors only");
        let routes = t.channels.routes().await.unwrap();
        let trouble = routes.iter().find(|r| r.kind.key == "pool_trouble").unwrap();
        assert_eq!((trouble.in_app, trouble.channels.clone()), (true, vec![mail.id]));
        let failed = routes.iter().find(|r| r.kind.key == "updates_failed").unwrap();
        assert_eq!(failed.channels, [mail.id, chat.id]);

        let choice = KindChoice { kind: "pool_trouble".into(), in_app: false, channels: vec![chat.id] };
        let routes = t.channels.set_routes(vec![choice], &alice(), &request()).await.unwrap();
        let trouble = routes.iter().find(|r| r.kind.key == "pool_trouble").unwrap();
        assert_eq!((trouble.in_app, trouble.channels.clone()), (false, vec![chat.id]));
        t.channels.deliver(&degraded(Level::Warning)).await;
        assert_eq!(sent_to(&t.sender)[1], "https://chat.example");
        assert_eq!(sent_to(&t.sender).len(), 2, "no longer by mail");
        let untouched = routes.iter().find(|r| r.kind.key == "updates_failed").unwrap();
        assert_eq!(untouched.channels, [mail.id, chat.id], "the other kinds keep theirs");

        // A test goes to its channel alone, whatever the routes.
        t.channels.test(mail.id, &alice(), &request()).await.unwrap();
        assert_eq!(sent_to(&t.sender)[2], "me@example.org");

        let unknown = KindChoice { kind: "weather".into(), in_app: true, channels: vec![] };
        let refused = t.channels.set_routes(vec![unknown], &alice(), &request()).await;
        assert!(matches!(refused, Err(ChannelError::Invalid(InvalidChannel::Kind(_)))), "{refused:?}");
        let elsewhere = KindChoice { kind: "pool_trouble".into(), in_app: true, channels: vec![99] };
        let refused = t.channels.set_routes(vec![elsewhere], &alice(), &request()).await;
        assert!(matches!(refused, Err(ChannelError::NotFound)), "{refused:?}");

        // A channel's choices go with it.
        t.channels.delete(chat.id, &alice(), &request()).await.unwrap();
        let again = KindChoice { kind: "scrub_finished".into(), in_app: true, channels: vec![mail.id] };
        t.channels.set_routes(vec![again], &alice(), &request()).await.unwrap();
        let stored = t.routes.0.lock().unwrap().clone();
        assert!(stored.channels.keys().all(|(id, _)| *id == mail.id), "{stored:?}");
        let updates = t.audit.0.lock().unwrap().iter().filter(|a| **a == "notify.routes_update").count();
        assert_eq!(updates, 2);
    }

    #[tokio::test]
    async fn a_kind_not_kept_in_zendiloos_still_goes_out() {
        let t = setup();
        let chat =
            t.channels.create(webhook("Chat", "https://chat.example"), &alice(), &request()).await.unwrap();
        let mut routes = Routes::default();
        routes.in_app.insert("snapshot_failed".into(), false);
        routes.channels.insert((chat.id, "snapshot_failed".into()), true);
        *t.routes.0.lock().unwrap() = routes;

        let stored = Arc::new(MemoryNotifications::default());
        let notifier = crate::Notifier::new(stored.clone(), Arc::new(Fixed)).routed(t.routes.clone());
        let mut browsers = notifier.subscribe();
        let delivering = t.channels.clone().spawn(&notifier);
        notifier
            .notify(crate::NewNotification::new(Level::Warning, "snapshot_failed").with("share", "photos"))
            .await;
        for _ in 0..100 {
            if !t.sender.sent.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(t.sender.sent.lock().unwrap()[0].1.title, "[nas] No snapshot of photos");
        assert!(stored.all().is_empty(), "not kept");
        assert!(browsers.try_recv().is_err(), "nor shown");

        notifier
            .notify(crate::NewNotification::new(Level::Warning, "pool_trouble").with("pool", "tank"))
            .await;
        assert_eq!(stored.all().len(), 1, "other kinds are kept");
        assert_eq!(browsers.recv().await.unwrap().code, "pool_trouble");
        delivering.abort();
    }

    #[tokio::test]
    async fn there_is_a_limit_of_channels_and_names_are_unique() {
        let t = setup();
        for n in 0..MAX_CHANNELS {
            let hook = webhook(&format!("Chat {n}"), &format!("https://x.example/{n}"));
            t.channels.create(hook, &alice(), &request()).await.unwrap();
        }
        let one_more = t.channels.create(webhook("More", "https://x.example"), &alice(), &request()).await;
        assert!(matches!(one_more, Err(ChannelError::TooMany)), "{one_more:?}");
        let missing = t.channels.update(99, webhook("Chat", "https://x.example"), &alice(), &request()).await;
        assert!(matches!(missing, Err(ChannelError::NotFound)), "{missing:?}");
        let listed = t.channels.list().await.unwrap();
        let taken = t
            .channels
            .update(listed[0].id, webhook("Chat 1", "https://y.example"), &alice(), &request())
            .await;
        assert!(matches!(taken, Err(ChannelError::NameTaken)), "{taken:?}");
        assert!(matches!(t.channels.delete(99, &alice(), &request()).await, Err(ChannelError::NotFound)));
        assert!(matches!(t.channels.test(99, &alice(), &request()).await, Err(ChannelError::NotFound)));
    }

    #[tokio::test]
    async fn a_change_keeps_the_channels_level_and_records_its_last_delivery() {
        let t = setup();
        let chat =
            t.channels.create(webhook("Chat", "https://broken.example"), &alice(), &request()).await.unwrap();
        let mut renamed = webhook("Team chat", "https://broken.example/2");
        renamed.min_level = Level::Info;
        let changed = t.channels.update(chat.id, renamed, &alice(), &request()).await.unwrap();
        assert_eq!((changed.settings.name.as_str(), changed.settings.min_level), ("Team chat", Level::Error));
        let failed = t.channels.test(chat.id, &alice(), &request()).await;
        assert!(
            matches!(failed, Err(ChannelError::Delivery(ref e)) if e == "connection refused"),
            "{failed:?}"
        );
        let listed = t.channels.list().await.unwrap();
        assert_eq!(listed[0].last.as_ref().and_then(|d| d.error.as_deref()), Some("connection refused"));
        assert_eq!(
            *t.audit.0.lock().unwrap(),
            ["notify.channel_create", "notify.channel_update", "notify.channel_test"]
        );
    }
}
