//! The web interface's TLS, served by nginx.

use serde::{Deserialize, Serialize};

/// The oldest TLS version the web interface accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlsMinimum {
    #[serde(rename = "tls1_2")]
    Tls12,
    #[serde(rename = "tls1_3")]
    Tls13,
}

impl TlsMinimum {
    /// nginx's `ssl_protocols` for it.
    pub fn nginx_protocols(self) -> &'static str {
        match self {
            TlsMinimum::Tls12 => "TLSv1.2 TLSv1.3",
            TlsMinimum::Tls13 => "TLSv1.3",
        }
    }
}
