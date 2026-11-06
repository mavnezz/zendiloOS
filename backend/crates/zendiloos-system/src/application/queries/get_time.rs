use std::sync::Arc;

use zendiloos_kernel::{PortError, RequestContext};

use crate::application::ports::TimeControl;
use crate::domain::TimeStatus;

/// Query: the time zone, how the clock is kept, and the zones there are.
#[derive(Clone)]
pub struct GetTime {
    time: Arc<dyn TimeControl>,
}

impl GetTime {
    pub fn new(time: Arc<dyn TimeControl>) -> Self {
        Self { time }
    }

    pub async fn execute(&self, request: &RequestContext) -> Result<TimeStatus, PortError> {
        self.time.status(request).await
    }
}
