use std::sync::Arc;

use zendiloos_kernel::{PortError, RequestContext};

use crate::application::ports::WebTlsControl;
use crate::domain::TlsMinimum;

/// Query: the oldest TLS version the web interface accepts.
#[derive(Clone)]
pub struct GetTls {
    tls: Arc<dyn WebTlsControl>,
}

impl GetTls {
    pub fn new(tls: Arc<dyn WebTlsControl>) -> Self {
        Self { tls }
    }

    pub async fn execute(&self, request: &RequestContext) -> Result<TlsMinimum, PortError> {
        self.tls.minimum(request).await
    }
}
