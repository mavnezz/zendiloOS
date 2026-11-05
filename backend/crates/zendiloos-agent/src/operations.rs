use std::time::Duration;

use zendiloos_agent_proto::{ErrorCode, PROTOCOL_VERSION, Request, Response};
use zendiloos_exec::{ExecError, Limits, Program};

use crate::config::Tools;

/// Carries out requests. Each operation runs fixed programs with fixed
/// arguments; values from a request reach a command line only as validated
/// protocol types.
#[derive(Debug)]
pub struct Operations {
    systemctl: Program,
}

impl Operations {
    pub fn new(tools: &Tools) -> Result<Self, ExecError> {
        Ok(Self { systemctl: Program::new(&tools.systemctl)? })
    }

    pub async fn execute(&self, request: &Request) -> Response {
        match request {
            Request::Hello {} => Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            // systemctl returns once the job is queued; the shutdown follows.
            Request::Reboot {} => self.systemctl("reboot").await,
            Request::PowerOff {} => self.systemctl("poweroff").await,
        }
    }

    async fn systemctl(&self, verb: &'static str) -> Response {
        let limits = Limits { timeout: Duration::from_secs(30), ..Limits::default() };
        match zendiloos_exec::run(&self.systemctl, [verb], limits)
            .await
            .and_then(|o| o.success(&self.systemctl))
        {
            Ok(_) => Response::Done,
            Err(error) => Response::Error { code: ErrorCode::Failed, message: error.to_string() },
        }
    }
}
