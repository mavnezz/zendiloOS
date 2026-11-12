use std::sync::Arc;

use zendiloos_kernel::{PortError, RequestContext};

use crate::application::ports::HostNameControl;

/// Query: the machine's name as the system keeps it.
#[derive(Clone)]
pub struct GetHostName {
    host_name: Arc<dyn HostNameControl>,
}

impl GetHostName {
    pub fn new(host_name: Arc<dyn HostNameControl>) -> Self {
        Self { host_name }
    }

    pub async fn execute(&self, request: &RequestContext) -> Result<String, PortError> {
        self.host_name.name(request).await
    }
}
