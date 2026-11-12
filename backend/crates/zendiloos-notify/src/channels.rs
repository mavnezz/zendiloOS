//! Channels that carry notifications beyond the interface, so that a
//! failing disk is noticed without a browser open: email and webhooks
//! (chat services, Gotify, ntfy, home automation).
//!
//! Mail goes out through one mail server, set once; an email channel names
//! only its recipients: zendiloOS accounts (at the address the account has
//! when a mail goes out) and other addresses. Which notifications a
//! channel carries is the routing table's business ([`crate::routes`]);
//! how a delivery that did not go out is tried again, the queue's
//! ([`crate::queue`]).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinHandle;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext, Timestamp};

use crate::message::{Language, Message, message};
pub use crate::queue::SendFailure;
use crate::queue::{
    ChannelQueue, DeliveryQueue, FAILED_KEPT, FAILED_KEPT_FOR, QueueState, QueuedDelivery, after_failure,
    by_channel,
};
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
    /// Delivers one message; the error in words an administrator can act
    /// on, and whether it may pass.
    async fn send(
        &self,
        target: &Target,
        message: &Message,
        notification: &Notification,
        host: &str,
    ) -> Result<(), SendFailure>;
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

/// A delivery that takes longer than this has failed, and may go out
/// when tried again.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the queue sleeps at most without anything due: its records
/// are looked at again, should a wake-up have gone missing.
const IDLE: Duration = Duration::from_secs(300);

/// What channels need from outside.
#[derive(Clone)]
pub struct ChannelPorts {
    pub repository: Arc<dyn ChannelRepository>,
    pub routes: Arc<dyn RouteRepository>,
    pub queue: Arc<dyn DeliveryQueue>,
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
    /// The NAS's name, in every message; it follows a rename.
    host: Arc<Mutex<String>>,
    /// Something was queued or a channel is done: the queue looks again.
    wake: Arc<Notify>,
    /// The channels whose deliveries are being sent, each by a task of its
    /// own, so a slow server holds up no other.
    sending: Arc<Mutex<HashSet<i64>>>,
}

/// Takes a channel off [`Channels::sending`] when its task ends, however
/// it ends, and has the queue look again.
struct Sending {
    channel_id: i64,
    sending: Arc<Mutex<HashSet<i64>>>,
    wake: Arc<Notify>,
}

impl Drop for Sending {
    fn drop(&mut self) {
        self.sending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).remove(&self.channel_id);
        self.wake.notify_one();
    }
}

impl Channels {
    pub fn new(ports: ChannelPorts, host: String) -> Self {
        Self {
            ports,
            host: Arc::new(Mutex::new(host)),
            wake: Arc::new(Notify::new()),
            sending: Arc::default(),
        }
    }

    /// The NAS was renamed: messages from now on name it so.
    pub fn rename(&self, host: &str) {
        host.clone_into(&mut self.host.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    }

    fn host(&self) -> String {
        self.host.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
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
        let sent = self.send(&channel, &notification).await;
        let outcome = if sent.is_ok() { Outcome::Success } else { Outcome::Failure };
        self.audit("notify.channel_test", outcome, &channel.settings.name, actor, request).await;
        if let Err(failure) = sent {
            return Err(ChannelError::Delivery(failure.reason));
        }
        // It reaches its receiver again: what waits for it goes out now.
        self.hurry(id).await;
        Ok(())
    }

    /// What waits on the channels and what failed for good, oldest first.
    pub async fn queued(&self) -> Result<Vec<QueuedDelivery>, ChannelError> {
        Ok(self.ports.queue.queued().await?)
    }

    /// Makes the deliveries waiting for `channel_id` due now.
    async fn hurry(&self, channel_id: i64) {
        let now = self.ports.clock.now();
        let waiting = match self.ports.queue.queued().await {
            Ok(listed) => listed,
            Err(error) => return tracing::warn!(%error, "channels: cannot read what waits"),
        };
        for delivery in
            waiting.iter().filter(|d| d.channel_id == channel_id && d.state == QueueState::Waiting)
        {
            if delivery.at > now
                && let Err(error) =
                    self.ports.queue.record(&QueuedDelivery { at: now, ..delivery.clone() }).await
            {
                tracing::warn!(%error, "channels: cannot bring a delivery forward");
            }
        }
        self.wake.notify_one();
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

    /// What `channel` delivers to now, or why it cannot deliver: settings
    /// that need a person fail for good, a database that cannot be read
    /// may pass.
    async fn target(&self, channel: &Channel) -> Result<Target, SendFailure> {
        match &channel.settings.destination {
            Destination::Webhook(webhook) => Ok(Target::Webhook(webhook.clone())),
            Destination::Email { .. } => {
                let unreadable = |error: PortError| SendFailure::transient(error.to_string());
                let server = self.ports.repository.mail_server().await.map_err(unreadable)?;
                let server = server.ok_or_else(|| SendFailure::permanent("no mail server is set up"))?;
                let states =
                    self.recipients(channel).await.map_err(|e| SendFailure::transient(e.to_string()))?;
                let to: Vec<String> = states
                    .into_iter()
                    .filter_map(|state| match state.address {
                        AccountAddress::Address(address) => Some(address),
                        AccountAddress::Missing | AccountAddress::Disabled | AccountAddress::Gone => None,
                    })
                    .collect();
                if to.is_empty() {
                    return Err(SendFailure::permanent("none of the recipients has an email address"));
                }
                Ok(Target::Email(server.mail(to)))
            }
        }
    }

    /// Sends `notification` on `channel` once, and records the outcome as
    /// the channel's last delivery.
    async fn send(&self, channel: &Channel, notification: &Notification) -> Result<(), SendFailure> {
        let host = self.host();
        let text = message(notification, &host, channel.settings.language);
        let sent = match self.target(channel).await {
            Ok(target) => tokio::time::timeout(
                SEND_TIMEOUT,
                self.ports.sender.send(&target, &text, notification, &host),
            )
            .await
            .unwrap_or_else(|_| {
                Err(SendFailure::transient(format!("no answer within {} seconds", SEND_TIMEOUT.as_secs())))
            }),
            Err(failure) => Err(failure),
        };
        let error = sent.as_ref().err().map(|failure| failure.reason.clone());
        let delivery = Delivery { at: self.ports.clock.now(), error };
        if let Err(error) = self.ports.repository.delivered(channel.id, &delivery).await {
            tracing::warn!(%error, "channels: cannot record a delivery");
        }
        sent
    }

    /// Queues `notification` for every channel its kind is routed to; the
    /// queue sends it at once. Should the queue not take it, it is sent
    /// once right away instead.
    pub async fn deliver(&self, notification: &Notification) {
        let (channels, routes) = match (self.ports.repository.list().await, self.ports.routes.routes().await)
        {
            (Ok(channels), Ok(routes)) => (channels, routes),
            (Err(error), _) | (_, Err(error)) => {
                return tracing::warn!(%error, "channels: cannot read them or their routes");
            }
        };
        let kind = kind_of(&notification.code, notification.level);
        let routed: Vec<&Channel> =
            channels.iter().filter(|c| carried(&routes, c, kind, notification.level)).collect();
        if routed.is_empty() {
            return;
        }
        let ids: Vec<i64> = routed.iter().map(|c| c.id).collect();
        match self.ports.queue.enqueue(notification, &ids, self.ports.clock.now()).await {
            Ok(()) => self.wake.notify_one(),
            Err(error) => {
                tracing::error!(%error, code = notification.code, "channels: cannot queue; sent once now");
                for channel in routed {
                    let sent = self.send(channel, notification).await;
                    log_outcome(channel, notification, &sent.map_err(|f| f.reason), "not delivered");
                }
            }
        }
    }

    /// The channels whose waiting deliveries are due, with those, and when
    /// the next of the others is due. A channel's deliveries go with it
    /// when it is removed; one this version cannot read keeps them.
    async fn due(&self) -> Result<(Vec<(Channel, ChannelQueue)>, Option<Timestamp>), PortError> {
        let now = self.ports.clock.now();
        let channels = self.ports.repository.list().await?;
        let mut due = Vec::new();
        let mut next: Option<Timestamp> = None;
        for queue in by_channel(self.ports.queue.queued().await?) {
            let Some(channel) = channels.iter().find(|c| c.id == queue.channel_id) else { continue };
            match queue.due_at() {
                Some(at) if at <= now => due.push((channel.clone(), queue)),
                Some(at) => next = Some(next.map_or(at, |next| next.min(at))),
                None => {}
            }
        }
        Ok((due, next))
    }

    /// Sends a channel's waiting deliveries, oldest first, until one fails
    /// in a way that may pass: it waits for its next try, and those behind
    /// it wait with it.
    async fn send_queue(&self, channel: &Channel, queue: ChannelQueue) {
        let mut waiting = queue.waiting.into_iter();
        while let Some(delivery) = waiting.next() {
            let notification = &delivery.notification;
            let sent = self.send(channel, notification).await;
            let recorded = match sent {
                Ok(()) => {
                    log_outcome(channel, notification, &Ok(()), "");
                    self.ports.queue.dequeue(delivery.id).await
                }
                Err(failure) => {
                    let after = after_failure(&delivery, &failure, self.ports.clock.now());
                    let outcome = Err(failure.reason);
                    if after.state == QueueState::Waiting {
                        log_outcome(channel, notification, &outcome, "not delivered, tried again later");
                        let behind = std::iter::once(after.clone())
                            .chain(waiting.by_ref().map(|d| QueuedDelivery { at: after.at, ..d }));
                        for delivery in behind {
                            if let Err(error) = self.ports.queue.record(&delivery).await {
                                tracing::warn!(%error, "channels: cannot record a delivery that waits");
                            }
                        }
                        return;
                    }
                    log_outcome(channel, notification, &outcome, "not delivered, given up");
                    let failed = self.ports.queue.record(&after).await;
                    let before = self.ports.clock.now().saturating_sub(FAILED_KEPT_FOR);
                    failed.and(self.ports.queue.prune(before, FAILED_KEPT).await)
                }
            };
            if let Err(error) = recorded {
                tracing::warn!(%error, "channels: cannot record a delivery");
            }
        }
    }

    /// Starts sending the due deliveries of every channel not being sent to
    /// already, each channel on a task of its own; when the next of the
    /// others is due.
    async fn start_due(&self) -> Option<Timestamp> {
        let (due, next) = match self.due().await {
            Ok(due) => due,
            Err(error) => {
                tracing::warn!(%error, "channels: cannot read what waits");
                return None;
            }
        };
        for (channel, queue) in due {
            if !self.sending.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(channel.id) {
                continue;
            }
            let guard =
                Sending { channel_id: channel.id, sending: self.sending.clone(), wake: self.wake.clone() };
            let channels = self.clone();
            tokio::spawn(async move {
                let _done = guard;
                channels.send_queue(&channel, queue).await;
            });
        }
        next
    }

    /// Sends what is due now, one channel after the other, and returns when
    /// that is done: what the queue's task does, for tests.
    #[cfg(any(test, feature = "testing"))]
    pub async fn send_due(&self) {
        if let Ok((due, _)) = self.due().await {
            for (channel, queue) in due {
                self.send_queue(&channel, queue).await;
            }
        }
    }

    /// Queues every notification `notifier` raises from now on, kept in
    /// zendiloOS or not, and sends what is queued: what is due at once,
    /// what waits when it is due, also what waited before a restart.
    pub fn spawn(self, notifier: &Notifier) -> JoinHandle<()> {
        let mut events = notifier.outgoing();
        let queue = self.clone();
        tokio::spawn(async move {
            // Stops with this task, also when it is aborted.
            let _sending = AbortOnDrop(tokio::spawn(async move {
                let before = queue.ports.clock.now().saturating_sub(FAILED_KEPT_FOR);
                if let Err(error) = queue.ports.queue.prune(before, FAILED_KEPT).await {
                    tracing::warn!(%error, "channels: cannot forget old failed deliveries");
                }
                loop {
                    let next = queue.start_due().await;
                    let now = queue.ports.clock.now();
                    let pause = next.map_or(IDLE, |at| at.duration_since(now).max(Duration::from_secs(1)));
                    let _ = tokio::time::timeout(pause.min(IDLE), queue.wake.notified()).await;
                }
            }));
            loop {
                match events.recv().await {
                    Ok(notification) => self.deliver(&notification).await,
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "channels: notifications came too fast, some were not sent");
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        })
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Logs how a delivery went.
fn log_outcome(channel: &Channel, notification: &Notification, sent: &Result<(), String>, failed: &str) {
    match sent {
        Ok(()) => {
            tracing::info!(channel = channel.settings.name, code = notification.code, "channels: delivered")
        }
        Err(error) => tracing::warn!(
            channel = channel.settings.name,
            code = notification.code,
            error,
            "channels: {failed}"
        ),
    }
}

fn carried(routes: &Routes, channel: &Channel, kind: &str, level: Level) -> bool {
    routes.to_channel(channel.id, channel.settings.min_level, kind, level)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;
    use crate::queue::{TRIES, pause};
    use crate::routes::Routes;
    use crate::testing::{
        MemoryAddresses, MemoryChannels, MemoryNotifications, MemoryQueue, MemoryRoutes, RecordingSender,
    };

    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(1_793_836_800)
        }
    }

    /// Starts when [`Fixed`] stands and goes forward when told.
    struct Moving(AtomicI64);

    impl Moving {
        fn new() -> Self {
            Self(AtomicI64::new(Fixed.now().unix_seconds()))
        }

        fn advance(&self, by: Duration) {
            self.0.fetch_add(by.as_secs() as i64, Ordering::SeqCst);
        }
    }

    impl Clock for Moving {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(self.0.load(Ordering::SeqCst))
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
        queue: Arc<MemoryQueue>,
        clock: Arc<Moving>,
    }

    fn setup() -> Setup {
        setup_with(Arc::new(MemoryQueue::default()))
    }

    fn setup_with(queue: Arc<MemoryQueue>) -> Setup {
        let sender = Arc::new(RecordingSender::default());
        let audit = Arc::new(Audit::default());
        let routes = Arc::new(MemoryRoutes::default());
        let addresses = Arc::new(MemoryAddresses::default());
        let clock = Arc::new(Moving::new());
        let channels = Channels::new(
            ChannelPorts {
                repository: Arc::new(MemoryChannels::default()),
                routes: routes.clone(),
                queue: queue.clone(),
                addresses: addresses.clone(),
                sender: sender.clone(),
                audit: audit.clone(),
                clock: clock.clone(),
            },
            "nas".into(),
        );
        Setup { channels, sender, audit, routes, addresses, queue, clock }
    }

    impl Setup {
        /// Raises `notification` and sends what is due.
        async fn deliver(&self, notification: &Notification) {
            self.channels.deliver(notification).await;
            self.channels.send_due().await;
        }

        fn fail(&self, failure: Option<SendFailure>) {
            *self.sender.failing.lock().unwrap() = failure;
        }

        /// The queue's deliveries: their code, state and tries.
        fn queued(&self) -> Vec<(String, QueueState, u32)> {
            let all = self.queue.0.lock().unwrap();
            all.iter().map(|d| (d.notification.code.clone(), d.state, d.attempts)).collect()
        }
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
        t.deliver(&degraded(Level::Warning)).await;
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

        // By default: by each channel's level and the kind's most serious
        // level. The chat carries errors only, so it gets all pool trouble
        // (which can be an error), but no data check that found errors
        // (never more than a warning).
        t.deliver(&degraded(Level::Warning)).await;
        assert_eq!(sent_to(&t.sender), ["me@example.org", "https://chat.example"]);
        let scrub = Notification { code: "scrub_finished".into(), ..degraded(Level::Warning) };
        t.deliver(&scrub).await;
        assert_eq!(sent_to(&t.sender)[2..], ["me@example.org"], "found errors: a warning");
        let routes = t.channels.routes().await.unwrap();
        let trouble = routes.iter().find(|r| r.kind.key == "pool_trouble").unwrap();
        assert_eq!((trouble.in_app, trouble.channels.clone()), (true, vec![mail.id, chat.id]));
        let errors = routes.iter().find(|r| r.kind.key == "scrub_errors").unwrap();
        assert_eq!(errors.channels, [mail.id]);
        let finished = routes.iter().find(|r| r.kind.key == "scrub_finished").unwrap();
        assert!(finished.channels.is_empty(), "a routine check goes nowhere by default");
        t.sender.sent.lock().unwrap().clear();

        let choice = KindChoice { kind: "pool_trouble".into(), in_app: false, channels: vec![chat.id] };
        let routes = t.channels.set_routes(vec![choice], &alice(), &request()).await.unwrap();
        let trouble = routes.iter().find(|r| r.kind.key == "pool_trouble").unwrap();
        assert_eq!((trouble.in_app, trouble.channels.clone()), (false, vec![chat.id]));
        t.deliver(&degraded(Level::Warning)).await;
        assert_eq!(sent_to(&t.sender), ["https://chat.example"], "no longer by mail");
        let untouched = routes.iter().find(|r| r.kind.key == "scrub_errors").unwrap();
        assert_eq!(untouched.channels, [mail.id], "the other kinds keep theirs");

        // A test goes to its channel alone, whatever the routes.
        t.channels.test(mail.id, &alice(), &request()).await.unwrap();
        assert_eq!(sent_to(&t.sender)[1], "me@example.org");

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

    /// A webhook that carries everything.
    async fn chat(t: &Setup) -> Channel {
        let all = ChannelSettings { min_level: Level::Info, ..webhook("Chat", "https://chat.example") };
        t.channels.create(all, &alice(), &request()).await.unwrap()
    }

    fn recovered() -> Notification {
        Notification {
            id: 8,
            code: "pool_recovered".into(),
            level: Level::Success,
            ..degraded(Level::Warning)
        }
    }

    fn sent_titles(sender: &RecordingSender) -> Vec<String> {
        sender.sent.lock().unwrap().iter().map(|(_, message)| message.title.clone()).collect()
    }

    #[tokio::test]
    async fn what_may_pass_is_tried_again_in_order_until_it_goes_out() {
        let t = setup();
        let chat = chat(&t).await;
        t.fail(Some(SendFailure::transient("Temporary failure in name resolution")));

        t.deliver(&degraded(Level::Error)).await;
        assert!(sent_to(&t.sender).is_empty());
        assert_eq!(t.queued(), [("pool_trouble".into(), QueueState::Waiting, 1)]);
        let listed = t.channels.queued().await.unwrap();
        assert_eq!(listed[0].at, Fixed.now().saturating_add(pause(1)), "a minute later");
        let last = t.channels.list().await.unwrap()[0].last.clone().unwrap();
        assert_eq!(last.error.as_deref(), Some("Temporary failure in name resolution"));

        // A new one for the channel tries the oldest at once, and waits
        // behind it.
        t.deliver(&recovered()).await;
        assert_eq!(
            t.queued(),
            [
                ("pool_trouble".into(), QueueState::Waiting, 2),
                ("pool_recovered".into(), QueueState::Waiting, 0)
            ]
        );
        let due = Fixed.now().saturating_add(pause(2));
        assert!(t.channels.queued().await.unwrap().iter().all(|d| d.at == due), "both wait two minutes");

        t.fail(None);
        t.channels.send_due().await;
        assert!(sent_to(&t.sender).is_empty(), "not before it is due");
        t.clock.advance(pause(2));
        t.channels.send_due().await;
        assert_eq!(
            sent_titles(&t.sender),
            ["[nas] Pool tank is degraded", "[nas] Pool tank is healthy again"]
        );
        assert!(t.queued().is_empty(), "gone out, gone from the queue");
        assert_eq!(t.channels.list().await.unwrap()[0].last.as_ref().unwrap().error, None);
        assert_eq!(chat.id, t.channels.list().await.unwrap()[0].id);
    }

    #[tokio::test]
    async fn what_cannot_succeed_fails_for_good_at_once() {
        let t = setup();
        chat(&t).await;
        t.fail(Some(SendFailure::permanent("the server answered 401 Unauthorized")));

        t.deliver(&degraded(Level::Warning)).await;
        assert_eq!(t.queued(), [("pool_trouble".into(), QueueState::Failed, 1)]);
        let failed = t.channels.queued().await.unwrap().remove(0);
        assert_eq!(
            (failed.at, failed.error.as_deref()),
            (Fixed.now(), Some("the server answered 401 Unauthorized"))
        );

        // It holds up nothing behind it.
        t.fail(None);
        t.deliver(&recovered()).await;
        assert_eq!(sent_titles(&t.sender), ["[nas] Pool tank is healthy again"]);
        assert_eq!(t.queued(), [("pool_trouble".into(), QueueState::Failed, 1)], "kept to be shown");

        // Settings that need a person do not pass by themselves either.
        let mail = email("Admins", vec![address("me@example.org")]);
        t.channels.create(mail, &alice(), &request()).await.unwrap();
        t.queue.0.lock().unwrap().clear();
        t.deliver(&degraded(Level::Warning)).await;
        let listed = t.channels.queued().await.unwrap();
        let mailed = listed.iter().find(|d| d.state == QueueState::Failed).unwrap();
        assert_eq!(mailed.error.as_deref(), Some("no mail server is set up"));
    }

    #[tokio::test]
    async fn a_delivery_is_given_up_when_its_tries_run_out() {
        let t = setup();
        chat(&t).await;
        t.fail(Some(SendFailure::transient("no answer within 30 seconds")));

        t.deliver(&degraded(Level::Warning)).await;
        for tries in 1..TRIES {
            assert_eq!(t.queued(), [("pool_trouble".into(), QueueState::Waiting, tries)]);
            t.clock.advance(pause(tries));
            t.channels.send_due().await;
        }
        assert_eq!(t.queued(), [("pool_trouble".into(), QueueState::Failed, TRIES)]);
        let waited = t.clock.now().duration_since(Fixed.now()).as_secs() / 3600;
        assert_eq!(waited, 12, "about twelve hours");

        // Failed ones are shown for a week: the old one is forgotten when
        // the next one fails.
        t.clock.advance(FAILED_KEPT_FOR + Duration::from_secs(60));
        t.fail(Some(SendFailure::permanent("the server answered 404 Not Found")));
        t.deliver(&recovered()).await;
        assert_eq!(t.queued(), [("pool_recovered".into(), QueueState::Failed, 1)]);
    }

    #[tokio::test]
    async fn a_test_that_goes_out_sends_what_waits() {
        let t = setup();
        let chat = chat(&t).await;
        t.fail(Some(SendFailure::transient("connection refused")));
        t.deliver(&degraded(Level::Warning)).await;
        let failed = t.channels.test(chat.id, &alice(), &request()).await;
        assert!(
            matches!(failed, Err(ChannelError::Delivery(ref e)) if e == "connection refused"),
            "{failed:?}"
        );

        t.fail(None);
        t.channels.test(chat.id, &alice(), &request()).await.unwrap();
        t.channels.send_due().await;
        assert_eq!(sent_titles(&t.sender), ["[nas] Test message", "[nas] Pool tank is degraded"]);
        assert!(t.queued().is_empty());

        // Renamed: the next one names the new name.
        t.channels.rename("office");
        t.channels.test(chat.id, &alice(), &request()).await.unwrap();
        assert_eq!(sent_titles(&t.sender)[2], "[office] Test message");
    }

    #[tokio::test]
    async fn what_waited_before_a_restart_goes_out() {
        let queue = Arc::new(MemoryQueue::default());
        let t = setup_with(queue.clone());
        let chat = chat(&t).await;
        crate::queue::DeliveryQueue::enqueue(&*queue, &degraded(Level::Warning), &[chat.id], Fixed.now())
            .await
            .unwrap();

        let notifier = crate::Notifier::new(Arc::new(MemoryNotifications::default()), Arc::new(Fixed));
        let delivering = t.channels.clone().spawn(&notifier);
        for _ in 0..100 {
            if t.queued().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(sent_to(&t.sender), ["https://chat.example"]);
        delivering.abort();
    }

    /// A queue that takes nothing in.
    struct Full;

    #[async_trait]
    impl DeliveryQueue for Full {
        async fn enqueue(&self, _: &Notification, _: &[i64], _: Timestamp) -> Result<(), PortError> {
            Err(PortError::new("queue", "disk full"))
        }
        async fn queued(&self) -> Result<Vec<QueuedDelivery>, PortError> {
            Err(PortError::new("queue", "disk full"))
        }
        async fn dequeue(&self, _: i64) -> Result<(), PortError> {
            Ok(())
        }
        async fn record(&self, _: &QueuedDelivery) -> Result<(), PortError> {
            Ok(())
        }
        async fn prune(&self, _: Timestamp, _: u32) -> Result<(), PortError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn without_a_queue_a_notification_is_sent_once() {
        let t = setup();
        chat(&t).await;
        let channels =
            Channels::new(ChannelPorts { queue: Arc::new(Full), ..t.channels.ports.clone() }, "nas".into());

        channels.deliver(&degraded(Level::Warning)).await;
        channels.send_due().await;

        assert_eq!(sent_to(&t.sender), ["https://chat.example"]);
    }
}
