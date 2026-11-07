//! An https server on 127.0.0.1 for the tests of what zendiloosd fetches
//! itself, with a certificate authority of its own that only the tests'
//! TLS configuration trusts. The keys are made for these tests alone.

use std::sync::Arc;

use base64::Engine as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CA: &str = "
MIIBnzCCAUWgAwIBAgIUGCNbQ8h7thaQ1r/Ripj9GZkoffEwCgYIKoZIzj0EAwIw
HDEaMBgGA1UEAwwRemVuZGlsb09TIHRlc3QgQ0EwIBcNMjYxMTA3MDkwOTA4WhgP
MjEyNjEwMTQwOTA5MDhaMBwxGjAYBgNVBAMMEXplbmRpbG9PUyB0ZXN0IENBMFkw
EwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE1ppAhYRSA7A9QCOlNBZJUmePXrq2dzct
xFbYFthucZPvq6f7PIst8xEvt4DL+gJxxrjJD0Mgu3Vp7GpbR094DaNjMGEwHQYD
VR0OBBYEFLgbWjwRFRvaTi7J9ka3DpxyU8pTMB8GA1UdIwQYMBaAFLgbWjwRFRva
Ti7J9ka3DpxyU8pTMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgIEMAoG
CCqGSM49BAMCA0gAMEUCIG+q8/34KsSSLwGYE+DODwDU8+89yuQFTSThMrxVCEk3
AiEA7z+cZJZxm4KTY0+LGgy6TfVBXD30WCSIE+8448bO4CE=";

/// For `localhost` and `127.0.0.1`.
const SERVER: &str = "
MIIBxzCCAW2gAwIBAgIUV6jduiBcYULgPuJPAZ2/p9rpFgYwCgYIKoZIzj0EAwIw
HDEaMBgGA1UEAwwRemVuZGlsb09TIHRlc3QgQ0EwIBcNMjYxMTA3MDkwOTA4WhgP
MjEyNjEwMTQwOTA5MDhaMBQxEjAQBgNVBAMMCWxvY2FsaG9zdDBZMBMGByqGSM49
AgEGCCqGSM49AwEHA0IABPsHy9jy96E/qNROI4G1c3Qkrw4FyGQidMyDOLuCVwTi
HgBtxw67mdl1q9ngZqnka43wVlRNRJdM1XefxzcaYPmjgZIwgY8wGgYDVR0RBBMw
EYIJbG9jYWxob3N0hwR/AAABMAwGA1UdEwEB/wQCMAAwDgYDVR0PAQH/BAQDAgeA
MBMGA1UdJQQMMAoGCCsGAQUFBwMBMB0GA1UdDgQWBBR7aRrWbyFBfTuxAajyv4Vk
gTleVzAfBgNVHSMEGDAWgBS4G1o8ERUb2k4uyfZGtw6cclPKUzAKBggqhkjOPQQD
AgNIADBFAiEAsbRDEBJmeoz8bk5jOKg1CPFv3gDx6FjkBK6cKqYN8b8CIDGf2UUi
i8jjnYUM5QYpuyQPHPu0Cyh0IlF952We3sKg";

const SERVER_KEY: &str = "
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgIEIA2qbnxcpMBLXU
E5V2g3RTqj1N1xqu0neObIsGe1+hRANCAAT7B8vY8vehP6jUTiOBtXN0JK8OBchk
InTMgzi7glcE4h4AbccOu5nZdavZ4Gap5GuN8FZUTUSXTNV3n8c3GmD5";

fn der(text: &str) -> Vec<u8> {
    let joined: String = text.split_whitespace().collect();
    base64::engine::general_purpose::STANDARD.decode(joined).expect("the test certificates are base64")
}

/// TLS for clients that trusts the test authority only.
pub(crate) fn client() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(der(CA))).expect("the test authority");
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(tls)
}

/// One request as the server read it.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub(crate) method: String,
    pub(crate) path: String,
    /// The header lines, as sent, lowercase.
    pub(crate) headers: String,
}

/// Serves https on 127.0.0.1 until the test ends; `answer` gives the whole
/// HTTP/1.1 response to each request. Returns the port.
pub(crate) async fn serve(answer: impl Fn(&Asked) -> Vec<u8> + Send + Sync + 'static) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der(SERVER_KEY)));
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(der(SERVER))], key)
        .expect("the test certificate");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let answer = Arc::new(answer);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, answer) = (acceptor.clone(), Arc::clone(&answer));
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(tcp).await else { return };
                let mut read = Vec::new();
                let mut buffer = [0u8; 4096];
                while !read.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => read.extend_from_slice(&buffer[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&read).into_owned();
                let (head, _) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
                let (line, headers) = head.split_once("\r\n").unwrap_or((head, ""));
                let mut parts = line.split(' ');
                let asked = Asked {
                    method: parts.next().unwrap_or_default().to_owned(),
                    path: parts.next().unwrap_or_default().to_owned(),
                    headers: headers.to_lowercase(),
                };
                let _ = stream.write_all(&answer(&asked)).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    port
}

/// A response with `status` (`200 OK`), headers and a body.
pub(crate) fn response(status: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}
