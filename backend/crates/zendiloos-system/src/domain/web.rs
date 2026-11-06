//! The web interface's TLS.

/// The oldest TLS version the web interface accepts. TLS 1.2 is what older
/// clients still need; 1.3 alone refuses them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TlsMinimum {
    #[default]
    Tls12,
    Tls13,
}

impl TlsMinimum {
    pub fn label(self) -> &'static str {
        match self {
            TlsMinimum::Tls12 => "TLS 1.2",
            TlsMinimum::Tls13 => "TLS 1.3",
        }
    }
}
