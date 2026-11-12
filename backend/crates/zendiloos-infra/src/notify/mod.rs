//! Delivering notifications by email (SMTP) and to webhooks (HTTP POST).
//! TLS is rustls with the system's certificate authorities
//! (`ca-certificates`); nothing is bundled.
//!
//! A failure says whether it may pass, so the queue knows whether to try
//! again: a server not reached (no network or name resolution yet after a
//! boot, refused, timed out, cut off) or one that answers "later" (an SMTP
//! 4xx, an HTTP 5xx) may; an answer that refuses (a password, an address,
//! the request) or a certificate that is not trusted does not.

mod webhook;

use std::sync::Arc;

use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message as Mail};
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncTransport, Tokio1Executor};
use zendiloos_notify::Notification;
use zendiloos_notify::channels::{ChannelSender, Email, Security, Target};
use zendiloos_notify::message::Message;
use zendiloos_notify::queue::SendFailure;

use crate::https::refused_by_tls;

/// [`ChannelSender`] over the network.
#[derive(Debug, Clone)]
pub struct NetworkChannels {
    tls: Arc<rustls::ClientConfig>,
}

impl NetworkChannels {
    /// Reads the system's certificate authorities once; without any, TLS
    /// fails with a message that says so.
    pub fn new() -> Self {
        Self { tls: crate::https::client_config() }
    }
}

impl Default for NetworkChannels {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ChannelSender for NetworkChannels {
    async fn send(
        &self,
        target: &Target,
        message: &Message,
        notification: &Notification,
        host: &str,
    ) -> Result<(), SendFailure> {
        match target {
            Target::Email(email) => send_mail(email, message, host).await,
            Target::Webhook(hook) => {
                webhook::post(Arc::clone(&self.tls), hook, message, notification, host).await
            }
        }
    }
}

async fn send_mail(email: &Email, message: &Message, host: &str) -> Result<(), SendFailure> {
    let mailbox = |address: &str| {
        address.parse::<Mailbox>().map_err(|e| SendFailure::permanent(format!("{address}: {e}")))
    };
    let mut builder = Mail::builder().from(mailbox(&email.from)?).subject(&message.title);
    for to in &email.to {
        builder = builder.to(mailbox(to)?);
    }
    let body = if message.body.is_empty() { message.title.clone() } else { message.body.clone() };
    let mail = builder
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| SendFailure::permanent(e.to_string()))?;

    let tls = || {
        TlsParameters::builder(email.server.clone())
            .build_rustls()
            .map_err(|e| SendFailure::permanent(format!("TLS: {e}")))
    };
    let tls = match email.security {
        Security::Tls => Tls::Wrapper(tls()?),
        Security::StartTls => Tls::Required(tls()?),
        Security::None => Tls::None,
    };
    let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&email.server)
        .port(email.port)
        .tls(tls)
        .hello_name(ClientId::Domain(host.to_owned()));
    if let (Some(username), Some(password)) = (&email.username, &email.password) {
        transport = transport.credentials(Credentials::new(username.clone(), password.clone()));
    }
    transport.build().send(mail).await.map(drop).map_err(|e| smtp_failure(&e))
}

/// Whether an SMTP error may pass: a 4xx answer (greylisting, a full
/// mailbox, the server busy) or a server not reached. A 5xx answer (a
/// password or an address refused), a TLS certificate that is not trusted,
/// and a server that cannot do what is asked of it (STARTTLS, a way to
/// sign in) do not.
fn smtp_failure(error: &lettre::transport::smtp::Error) -> SendFailure {
    let reason = error.to_string();
    let lasting = error.is_permanent()
        || error.is_client()
        || error.is_response()
        || error.is_tls()
        || refused_by_tls(error);
    if error.is_transient() || error.is_timeout() || !lasting {
        SendFailure::transient(reason)
    } else {
        SendFailure::permanent(reason)
    }
}

#[cfg(test)]
mod tests;
