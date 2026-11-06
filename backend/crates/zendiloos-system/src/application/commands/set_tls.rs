use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::application::ports::WebTlsControl;
use crate::domain::TlsMinimum;

/// Command: sets the oldest TLS version the web interface accepts.
/// Connections already open stay. The caller has checked the permission.
#[derive(Clone)]
pub struct SetTls {
    tls: Arc<dyn WebTlsControl>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl SetTls {
    pub fn new(tls: Arc<dyn WebTlsControl>, audit: Arc<dyn AuditLog>, clock: Arc<dyn Clock>) -> Self {
        Self { tls, audit, clock }
    }

    pub async fn execute(
        &self,
        minimum: TlsMinimum,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<TlsMinimum, PortError> {
        let applied = self.tls.apply(minimum, request).await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        let entry = AuditEntry::new(self.clock.now(), "system.tls_update", outcome, request)
            .by(actor)
            .target(minimum.label());
        self.audit.record(entry).await?;
        applied?;
        self.tls.minimum(request).await
    }
}
