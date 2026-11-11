//! Against small servers on the loopback interface: an SMTP server that
//! accepts one message, an HTTP server that answers one request.

use std::collections::BTreeMap;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use zendiloos_kernel::Timestamp;
use zendiloos_notify::Level;
use zendiloos_notify::channels::{Webhook, WebhookFormat};
use zendiloos_notify::queue::SendFailure;

use super::*;

fn notification() -> Notification {
    Notification {
        id: 1,
        at: Timestamp::from_unix_seconds(1_793_836_800),
        level: Level::Warning,
        code: "pool_trouble".into(),
        params: BTreeMap::from([("pool".into(), "tank".into())]),
    }
}

fn message() -> Message {
    Message { title: "[nas] Pool tank ist beeinträchtigt".into(), body: "Unter Speicher nachsehen.".into() }
}

/// Accepts one message and returns everything the client said.
async fn smtp_server(listener: TcpListener) -> String {
    smtp_server_answering(listener, b"250 ok\r\n").await
}

/// As [`smtp_server`], answering `rcpt` to the recipient.
async fn smtp_server_answering(listener: TcpListener, rcpt: &'static [u8]) -> String {
    let (stream, _) = listener.accept().await.unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let mut said = String::new();
    writer.write_all(b"220 test ESMTP\r\n").await.unwrap();
    let mut in_data = false;
    while let Some(line) = lines.next_line().await.unwrap() {
        said.push_str(&line);
        said.push('\n');
        let reply: &[u8] = if in_data {
            if line != "." {
                continue;
            }
            in_data = false;
            b"250 queued\r\n"
        } else if line.starts_with("EHLO") {
            b"250 test\r\n"
        } else if line == "DATA" {
            in_data = true;
            b"354 go on\r\n"
        } else if line == "QUIT" {
            writer.write_all(b"221 bye\r\n").await.unwrap();
            break;
        } else if line.starts_with("RCPT") {
            rcpt
        } else {
            b"250 ok\r\n"
        };
        writer.write_all(reply).await.unwrap();
    }
    said
}

#[tokio::test]
async fn mail_goes_out_over_smtp() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(smtp_server(listener));
    let email = Email {
        server: "127.0.0.1".into(),
        port,
        security: Security::None,
        username: None,
        password: None,
        from: "nas@example.org".into(),
        to: vec!["admin@example.org".into()],
    };

    NetworkChannels::new().send(&Target::Email(email), &message(), &notification(), "nas").await.unwrap();

    let said = server.await.unwrap();
    assert!(said.starts_with("EHLO nas\n"), "{said}");
    assert!(said.contains("RCPT TO:<admin@example.org>"), "{said}");
    assert!(said.contains("Subject: [nas] Pool tank ist =?utf-8?b?"), "the umlaut is encoded: {said}");
}

#[tokio::test]
async fn starttls_is_required_when_chosen() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // The server offers no STARTTLS.
    tokio::spawn(smtp_server(listener));
    let email = Email {
        server: "127.0.0.1".into(),
        port,
        security: Security::StartTls,
        username: Some("nas".into()),
        password: Some("secret".into()),
        from: "nas@example.org".into(),
        to: vec!["admin@example.org".into()],
    };

    let refused =
        NetworkChannels::new().send(&Target::Email(email), &message(), &notification(), "nas").await;

    let refused = refused.expect_err("the password must not go out in clear");
    assert!(!refused.transient, "a server without STARTTLS stays one: {refused:?}");
}

fn plain_mail(port: u16) -> Target {
    Target::Email(Email {
        server: "127.0.0.1".into(),
        port,
        security: Security::None,
        username: None,
        password: None,
        from: "nas@example.org".into(),
        to: vec!["admin@example.org".into()],
    })
}

#[tokio::test]
async fn an_smtp_server_that_says_later_is_tried_again_and_one_that_refuses_is_not() {
    for (rcpt, transient) in [
        (&b"451 4.7.1 greylisted, try again later\r\n"[..], true),
        (&b"550 5.1.1 no such user\r\n"[..], false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(smtp_server_answering(listener, rcpt));

        let failed = NetworkChannels::new().send(&plain_mail(port), &message(), &notification(), "nas").await;

        let failed = failed.unwrap_err();
        assert_eq!(failed.transient, transient, "{failed:?}");
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let failed = NetworkChannels::new().send(&plain_mail(port), &message(), &notification(), "nas").await;
    assert!(failed.unwrap_err().transient, "a server not reached may come up");
}

/// Answers one request with `status` and returns it, head and body.
async fn http_server(listener: TcpListener, status: &'static str) -> String {
    let (mut stream, _) = listener.accept().await.unwrap();
    let mut request = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        request.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&request).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(str::to_owned))
                .and_then(|l| l.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= length {
                break;
            }
        }
        if read == 0 {
            break;
        }
    }
    stream.write_all(format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\n\r\n").as_bytes()).await.unwrap();
    String::from_utf8_lossy(&request).into_owned()
}

#[tokio::test]
async fn webhooks_get_json_or_text() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hooks/nas?token=1", listener.local_addr().unwrap());
    let server = tokio::spawn(http_server(listener, "204 No Content"));
    let hook = Target::Webhook(Webhook { url, format: WebhookFormat::Json });

    NetworkChannels::new().send(&hook, &message(), &notification(), "nas").await.unwrap();

    let request = server.await.unwrap();
    assert!(request.starts_with("POST /hooks/nas?token=1 HTTP/1.1\r\n"), "{request}");
    let body: serde_json::Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["title"], "[nas] Pool tank ist beeinträchtigt");
    assert_eq!(body["text"], "[nas] Pool tank ist beeinträchtigt\nUnter Speicher nachsehen.");
    assert_eq!((body["level"].as_str(), body["params"]["pool"].as_str()), (Some("warning"), Some("tank")));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let url = format!("http://nas:p%40ss@{address}/nas");
    let server = tokio::spawn(http_server(listener, "403 Forbidden"));
    let ntfy = Target::Webhook(Webhook { url, format: WebhookFormat::Text });

    let refused = NetworkChannels::new().send(&ntfy, &message(), &notification(), "nas").await;

    assert_eq!(refused, Err(SendFailure::permanent("the server answered 403 Forbidden")));
    let request = server.await.unwrap();
    assert!(request.contains("title: =?UTF-8?B?"), "{request}");
    assert!(request.contains(&format!("host: {address}\r\n")), "no user in Host: {request}");
    assert!(request.contains("authorization: Basic bmFzOnBAc3M=\r\n"), "{request}");
    assert!(request.ends_with("\r\n\r\nUnter Speicher nachsehen."), "{request}");
}

#[tokio::test]
async fn an_unreachable_server_says_so() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    let hook = Target::Webhook(Webhook { url, format: WebhookFormat::Json });

    let failed = NetworkChannels::new().send(&hook, &message(), &notification(), "nas").await.unwrap_err();

    assert!(failed.reason.starts_with("cannot connect to 127.0.0.1:"), "{failed:?}");
    assert!(failed.transient, "it may come up");
}

#[tokio::test]
async fn a_webhook_that_is_down_for_now_is_tried_again() {
    for (status, transient) in [
        ("503 Service Unavailable", true),
        ("502 Bad Gateway", true),
        ("429 Too Many Requests", true),
        ("404 Not Found", false),
        ("401 Unauthorized", false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(http_server(listener, status));
        let hook = Target::Webhook(Webhook { url, format: WebhookFormat::Json });

        let failed =
            NetworkChannels::new().send(&hook, &message(), &notification(), "nas").await.unwrap_err();

        assert_eq!(failed, SendFailure { reason: format!("the server answered {status}"), transient });
    }
}

#[test]
fn a_certificate_refused_stays_refused_a_connection_cut_off_does_not() {
    let refused = std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        rustls::Error::InvalidCertificate(rustls::CertificateError::Expired),
    );
    assert!(refused_by_tls(&refused));
    let cut_off = std::io::Error::from(std::io::ErrorKind::ConnectionReset);
    assert!(!refused_by_tls(&cut_off));
}
