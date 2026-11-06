//! A small https client over hyper and rustls with the system's
//! certificate authorities (`ca-certificates`), for what zendiloosd fetches
//! itself: app catalogs and their icons.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Limited};
use hyper::body::Bytes;
use hyper::header::{HOST, LOCATION, USER_AGENT};
use hyper::{Request, Uri};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// TLS for clients: rustls with ring and the system's certificate
/// authorities, read once.
pub fn client_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for error in &found.errors {
        tracing::warn!(%error, "tls: cannot read a certificate authority");
    }
    let (added, _) = roots.add_parsable_certificates(found.certs);
    if added == 0 {
        tracing::warn!("tls: no certificate authorities found; is ca-certificates installed?");
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
        .unwrap_or_else(|_| unreachable!("ring supports the default protocol versions"));
    Arc::new(tls)
}

/// Redirects followed; each must stay on https.
const MAX_REDIRECTS: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(30);

/// GETs `address` (https only), following a few redirects to https, and
/// returns at most `max_bytes` of the body; more is an error.
pub async fn get(
    tls: &Arc<rustls::ClientConfig>,
    address: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    tokio::time::timeout(TIMEOUT, follow(tls, address, max_bytes))
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {} seconds", TIMEOUT.as_secs())))
}

async fn follow(tls: &Arc<rustls::ClientConfig>, address: &str, max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut address = address.to_owned();
    for _ in 0..=MAX_REDIRECTS {
        let uri: Uri = address.parse().map_err(|e| format!("the address: {e}"))?;
        if uri.scheme_str() != Some("https") {
            return Err("only https addresses".into());
        }
        let authority = uri.authority().ok_or("the address has no host")?.clone();
        if authority.as_str().contains('@') {
            return Err("an address with a user is not fetched".into());
        }
        let host = authority.host().trim_start_matches('[').trim_end_matches(']').to_owned();
        let port = authority.port_u16().unwrap_or(443);
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| format!("cannot connect to {host}: {e}"))?;
        let name = ServerName::try_from(host.clone()).map_err(|_| format!("{host} is not a host name"))?;
        let stream =
            TlsConnector::from(Arc::clone(tls)).connect(name, tcp).await.map_err(|e| format!("TLS: {e}"))?;
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(stream)).await.map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let host_header = match authority.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.clone(),
        };
        let path = uri.path_and_query().map_or("/", |p| p.as_str());
        let request = Request::get(path)
            .header(HOST, host_header)
            .header(USER_AGENT, concat!("zendiloOS/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())
            .map_err(|e| e.to_string())?;
        let response = sender.send_request(request).await.map_err(|e| e.to_string())?;
        let status = response.status();
        if status.is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|l| l.to_str().ok())
                .ok_or("a redirect without a target")?;
            address = if location.starts_with('/') {
                format!("https://{}{location}", authority.as_str())
            } else {
                location.to_owned()
            };
            continue;
        }
        if !status.is_success() {
            return Err(format!("the server answered {status}"));
        }
        let body = Limited::new(response.into_body(), max_bytes)
            .collect()
            .await
            .map_err(|_| format!("larger than {} KiB", max_bytes / 1024))?;
        return Ok(body.to_bytes().to_vec());
    }
    Err("too many redirects".into())
}
