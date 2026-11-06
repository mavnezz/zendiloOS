use std::sync::Arc;

use zendiloos_kernel::{PortError, RequestContext};

use crate::application::ports::SshControl;
use crate::domain::SshStatus;

/// Query: whether the SSH server runs, on which port, and until when.
#[derive(Clone)]
pub struct GetSsh {
    ssh: Arc<dyn SshControl>,
}

impl GetSsh {
    pub fn new(ssh: Arc<dyn SshControl>) -> Self {
        Self { ssh }
    }

    pub async fn execute(&self, request: &RequestContext) -> Result<SshStatus, PortError> {
        self.ssh.status(request).await
    }
}
