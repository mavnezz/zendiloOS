//! A small https client over hyper and rustls with the system's
//! certificate authorities (`ca-certificates`), for what zendiloosd fetches
//! itself: app catalogs and their icons, and image digests from registries.

use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty, Limited};
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, HOST, HeaderMap, HeaderName, LOCATION, USER_AGENT};
use hyper::{Method, Request, StatusCode, Uri};
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
    let answer = send(tls, Method::GET, address, &[], max_bytes).await?;
    if !answer.status.is_success() {
        return Err(format!("the server answered {}", answer.status));
    }
    Ok(answer.body)
}

/// What a server answered.
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// Sends `method` to `address` (https only) with `headers`, following a few
/// redirects to https; an `Authorization` goes along only on the same host.
/// At most `max_bytes` of the body; more is an error.
pub async fn send(
    tls: &Arc<rustls::ClientConfig>,
    method: Method,
    address: &str,
    headers: &[(HeaderName, String)],
    max_bytes: usize,
) -> Result<Answer, String> {
    send_within(tls, method, address, headers, max_bytes, TIMEOUT).await
}

/// As [`send`], taking up to `within` for all of it: for large files.
pub async fn send_within(
    tls: &Arc<rustls::ClientConfig>,
    method: Method,
    address: &str,
    headers: &[(HeaderName, String)],
    max_bytes: usize,
    within: Duration,
) -> Result<Answer, String> {
    tokio::time::timeout(within, follow(tls, method, address, headers, max_bytes))
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {} seconds", within.as_secs())))
}

async fn follow(
    tls: &Arc<rustls::ClientConfig>,
    method: Method,
    address: &str,
    headers: &[(HeaderName, String)],
    max_bytes: usize,
) -> Result<Answer, String> {
    let mut address = address.to_owned();
    let mut first_host: Option<String> = None;
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
        let same_host = first_host.get_or_insert_with(|| host.clone()) == &host;
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
        let mut request = Request::builder()
            .method(method.clone())
            .uri(path)
            .header(HOST, host_header)
            .header(USER_AGENT, format!("zendiloOS/{}", zendiloos_kernel::VERSION));
        for (name, value) in headers {
            if *name == AUTHORIZATION && !same_host {
                continue;
            }
            request = request.header(name, value);
        }
        let request = request.body(Empty::<Bytes>::new()).map_err(|e| e.to_string())?;
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
        let headers = response.headers().clone();
        let body = Limited::new(response.into_body(), max_bytes)
            .collect()
            .await
            .map_err(|_| format!("larger than {} KiB", max_bytes / 1024))?;
        return Ok(Answer { status, headers, body: body.to_bytes().to_vec() });
    }
    Err("too many redirects".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_https::{Asked, client, response, serve};

    async fn server() -> u16 {
        serve(|asked: &Asked| match asked.path.as_str() {
            "/hello" => response("200 OK", &[], b"hello"),
            "/missing" => response("404 Not Found", &[], b""),
            "/moved" => response("302 Found", &[("Location", "/hello")], b""),
            "/elsewhere" => response("302 Found", &[("Location", "http://127.0.0.1/hello")], b""),
            "/nowhere" => response("302 Found", &[], b""),
            "/again" => response("302 Found", &[("Location", "/again")], b""),
            "/big" => response("200 OK", &[], &[b'x'; 4096]),
            "/who" => {
                let authorized = asked.headers.contains("authorization: bearer secret");
                response("200 OK", &[], if authorized { b"with" } else { b"without" })
            }
            other if other.starts_with("/to-localhost/") => {
                let port = other.trim_start_matches("/to-localhost/");
                let target = format!("https://localhost:{port}/who");
                response("302 Found", &[("Location", target.as_str())], b"")
            }
            _ => response("500 Internal Server Error", &[], b""),
        })
        .await
    }

    #[tokio::test]
    async fn fetches_over_https_and_follows_redirects_on_https() {
        let port = server().await;
        let tls = client();
        let at = |path: &str| format!("https://127.0.0.1:{port}{path}");
        assert_eq!(get(&tls, &at("/hello"), 1024).await.unwrap(), b"hello");
        assert_eq!(get(&tls, &at("/moved"), 1024).await.unwrap(), b"hello", "a relative redirect");
        let missing = get(&tls, &at("/missing"), 1024).await.unwrap_err();
        assert!(missing.contains("404"), "{missing}");
        let answer = send(&tls, Method::GET, &at("/hello"), &[], 64).await.unwrap();
        assert_eq!(answer.status, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_what_it_must_not_fetch() {
        let port = server().await;
        let tls = client();
        let at = |path: &str| format!("https://127.0.0.1:{port}{path}");
        let fails = |result: Result<Vec<u8>, String>, says: &str| {
            let error = result.unwrap_err();
            assert!(error.contains(says), "{error} does not say {says}");
        };
        fails(get(&tls, &format!("http://127.0.0.1:{port}/hello"), 1024).await, "only https");
        fails(get(&tls, &at("/elsewhere"), 1024).await, "only https");
        fails(get(&tls, &at("/nowhere"), 1024).await, "without a target");
        fails(get(&tls, &at("/again"), 1024).await, "too many redirects");
        fails(get(&tls, &at("/big"), 1024).await, "larger than 1 KiB");
        fails(get(&tls, &format!("https://user@127.0.0.1:{port}/hello"), 1024).await, "with a user");
        fails(get(&tls, "not an address", 1024).await, "");
        // The system's authorities do not know the test's.
        fails(get(&client_config(), &at("/hello"), 1024).await, "TLS");
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        fails(get(&tls, &format!("https://127.0.0.1:{closed}/hello"), 1024).await, "cannot connect");
    }

    #[tokio::test]
    async fn an_authorization_goes_to_the_first_host_only() {
        let port = server().await;
        let tls = client();
        let headers = [(AUTHORIZATION, "Bearer secret".to_owned())];
        let direct =
            send(&tls, Method::GET, &format!("https://127.0.0.1:{port}/who"), &headers, 64).await.unwrap();
        assert_eq!(direct.body, b"with");
        let redirected = format!("https://127.0.0.1:{port}/to-localhost/{port}");
        let moved = send(&tls, Method::GET, &redirected, &headers, 64).await.unwrap();
        assert_eq!(moved.body, b"without", "another host gets no credentials");
    }

    #[tokio::test]
    async fn gives_up_after_its_time() {
        // A server that takes connections and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let address = format!("https://127.0.0.1:{port}/slow");
        let error = send_within(&client(), Method::GET, &address, &[], 64, Duration::from_millis(200)).await;
        assert!(error.is_err_and(|e| e.contains("no answer within")));
    }
}
