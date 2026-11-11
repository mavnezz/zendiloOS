//! A webhook: one HTTP POST, over hyper's own client; redirects are not
//! followed.

use std::sync::Arc;

use base64::Engine;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST, HeaderValue, USER_AGENT};
use hyper::{Request, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zendiloos_notify::Notification;
use zendiloos_notify::channels::{Webhook, WebhookFormat};
use zendiloos_notify::message::Message;
use zendiloos_notify::queue::SendFailure;

/// What a webhook in JSON receives: the fields chat services, Gotify and
/// home automation look for.
pub(super) fn json_payload(message: &Message, notification: &Notification, host: &str) -> serde_json::Value {
    let text = if message.body.is_empty() {
        message.title.clone()
    } else {
        format!("{}\n{}", message.title, message.body)
    };
    serde_json::json!({
        "title": message.title,
        "message": if message.body.is_empty() { &message.title } else { &message.body },
        "text": text,
        "content": text,
        "level": notification.level.as_str(),
        "code": notification.code,
        "params": notification.params,
        "host": host,
        "at": notification.at.unix_seconds(),
    })
}

/// A header value for any text: as is if plain ASCII, otherwise as an
/// RFC 2047 encoded word (ntfy reads those).
fn header_text(text: &str) -> HeaderValue {
    let printable = text.chars().all(|c| c.is_ascii() && !c.is_ascii_control());
    let value = if printable {
        text.to_owned()
    } else {
        format!("=?UTF-8?B?{}?=", base64::engine::general_purpose::STANDARD.encode(text))
    };
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("zendiloOS"))
}

/// `%XX` in the user part of an address, as bytes.
fn percent_decoded(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                i += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    decoded
}

pub(super) async fn post(
    tls: Arc<rustls::ClientConfig>,
    hook: &Webhook,
    message: &Message,
    notification: &Notification,
    host: &str,
) -> Result<(), SendFailure> {
    let uri: Uri = hook.url.parse().map_err(|e| SendFailure::permanent(format!("the address: {e}")))?;
    let https = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err(SendFailure::permanent("only http:// and https:// addresses")),
    };
    let authority = uri.authority().ok_or_else(|| SendFailure::permanent("the address has no host"))?.clone();
    let server = authority.host().trim_start_matches('[').trim_end_matches(']').to_owned();
    let port = authority.port_u16().unwrap_or(if https { 443 } else { 80 });

    let (content_type, body) = match hook.format {
        WebhookFormat::Json => ("application/json", json_payload(message, notification, host).to_string()),
        WebhookFormat::Text => {
            let text = if message.body.is_empty() { message.title.clone() } else { message.body.clone() };
            ("text/plain; charset=utf-8", text)
        }
    };
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    // Host is the host and port only; user and password in the address
    // (`https://user:secret@host/`) become basic authentication.
    let host_header = match authority.port() {
        Some(port) => format!("{}:{port}", authority.host()),
        None => authority.host().to_owned(),
    };
    let mut request = Request::post(path)
        .header(HOST, host_header)
        .header(CONTENT_TYPE, content_type)
        .header(USER_AGENT, format!("zendiloOS/{}", zendiloos_kernel::VERSION));
    if hook.format == WebhookFormat::Text {
        request = request.header("Title", header_text(&message.title));
    }
    if let Some((user_info, _)) = authority.as_str().rsplit_once('@') {
        let credentials = base64::engine::general_purpose::STANDARD.encode(percent_decoded(user_info));
        request = request.header(hyper::header::AUTHORIZATION, format!("Basic {credentials}"));
    }
    let request =
        request.body(Full::new(Bytes::from(body))).map_err(|e| SendFailure::permanent(e.to_string()))?;

    // Name resolution and the connection: whatever goes wrong may pass.
    let tcp = TcpStream::connect((server.as_str(), port))
        .await
        .map_err(|e| SendFailure::transient(format!("cannot connect to {server}:{port}: {e}")))?;
    let status = if https {
        let name = ServerName::try_from(server.clone())
            .map_err(|_| SendFailure::permanent(format!("{server} is not a host name")))?;
        let stream = TlsConnector::from(tls).connect(name, tcp).await.map_err(|e| {
            let reason = format!("TLS: {e}");
            if super::refused_by_tls(&e) {
                SendFailure::permanent(reason)
            } else {
                SendFailure::transient(reason)
            }
        })?;
        exchange(TokioIo::new(stream), request).await?
    } else {
        exchange(TokioIo::new(tcp), request).await?
    };
    match status {
        status if status.is_success() => Ok(()),
        status => Err(answered(status)),
    }
}

/// A server that answers 5xx is down or overloaded for now, and 408 and
/// 429 ask to come back later; any other answer refuses this request, and
/// would refuse it again.
fn answered(status: StatusCode) -> SendFailure {
    let reason = format!("the server answered {status}");
    if crate::https::answered_later(status) {
        SendFailure::transient(reason)
    } else {
        SendFailure::permanent(reason)
    }
}

/// A connection cut off or closed may pass; an answer that is not HTTP
/// means the address leads somewhere else.
fn exchange_failure(error: &hyper::Error) -> SendFailure {
    let reason = error.to_string();
    if error.is_parse() || error.is_user() {
        SendFailure::permanent(reason)
    } else {
        SendFailure::transient(reason)
    }
}

async fn exchange<T>(io: T, request: Request<Full<Bytes>>) -> Result<StatusCode, SendFailure>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(io).await.map_err(|e| exchange_failure(&e))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let response = sender.send_request(request).await.map_err(|e| exchange_failure(&e))?;
    Ok(response.status())
}
