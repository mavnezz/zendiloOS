//! Delivering notifications by email (SMTP) and to webhooks (HTTP POST).
//! TLS is rustls with the system's certificate authorities
//! (`ca-certificates`); nothing is bundled.

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

/// [`ChannelSender`] over the network.
#[derive(Debug, Clone)]
pub struct NetworkChannels {
    tls: Arc<rustls::ClientConfig>,
}

impl NetworkChannels {
    /// Reads the system's certificate authorities once; without any, TLS
    /// fails with a message that says so.
    pub fn new() -> Self {
        let mut roots = rustls::RootCertStore::empty();
        let found = rustls_native_certs::load_native_certs();
        for error in &found.errors {
            tracing::warn!(%error, "channels: cannot read a certificate authority");
        }
        let (added, _) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            tracing::warn!("channels: no certificate authorities found; is ca-certificates installed?");
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
            .unwrap_or_else(|_| unreachable!("ring supports the default protocol versions"));
        Self { tls: Arc::new(tls) }
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
    ) -> Result<(), String> {
        match target {
            Target::Email(email) => send_mail(email, message, host).await,
            Target::Webhook(hook) => {
                webhook::post(Arc::clone(&self.tls), hook, message, notification, host).await
            }
        }
    }
}

async fn send_mail(email: &Email, message: &Message, host: &str) -> Result<(), String> {
    let mailbox = |address: &str| address.parse::<Mailbox>().map_err(|e| format!("{address}: {e}"));
    let mut builder = Mail::builder().from(mailbox(&email.from)?).subject(&message.title);
    for to in &email.to {
        builder = builder.to(mailbox(to)?);
    }
    let body = if message.body.is_empty() { message.title.clone() } else { message.body.clone() };
    let mail = builder.header(ContentType::TEXT_PLAIN).body(body).map_err(|e| e.to_string())?;

    let tls = || TlsParameters::builder(email.server.clone()).build_rustls().map_err(|e| format!("TLS: {e}"));
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
    transport.build().send(mail).await.map(drop).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
