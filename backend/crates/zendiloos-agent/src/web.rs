//! The web interface's TLS: the oldest version nginx accepts, written to
//! `/etc/zendiloos/nginx/tls.conf`, which nginx's site for zendiloOS
//! includes. nginx is reloaded; connections already open stay.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use zendiloos_agent_proto::{Response, TlsMinimum};
use zendiloos_exec::{Limits, Program};

use crate::config::Tools;
use crate::operations::OperationError;

const LIMITS: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 64 * 1024 };
const HEADER: &str = "# Written by zendiloOS; changes made here are replaced.\n";

/// Where the settings are written; other than the real place in tests.
#[derive(Debug, Clone)]
pub struct WebPaths {
    pub tls: PathBuf,
}

impl Default for WebPaths {
    fn default() -> Self {
        Self { tls: PathBuf::from("/etc/zendiloos/nginx/tls.conf") }
    }
}

/// The settings file for `minimum`.
pub fn tls_config(minimum: TlsMinimum) -> String {
    format!("{HEADER}ssl_protocols {};\n", minimum.nginx_protocols())
}

#[derive(Debug)]
pub(crate) struct Web {
    systemctl: Program,
    paths: WebPaths,
}

impl Web {
    pub(crate) fn new(tools: &Tools, paths: WebPaths) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self { systemctl: Program::new(&tools.systemctl)?, paths })
    }

    pub(crate) fn state(&self) -> Result<Response, OperationError> {
        let config = match fs::read_to_string(&self.paths.tls) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(OperationError::Read(self.paths.tls.clone(), error)),
        };
        Ok(Response::WebTlsState { config })
    }

    pub(crate) async fn set_tls(&self, minimum: TlsMinimum) -> Result<Response, OperationError> {
        let path = &self.paths.tls;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| OperationError::Write(parent.to_owned(), e))?;
        }
        crate::durable::replace(path, tls_config(minimum).as_bytes())?;
        // Not running (a container, the simulator's host): nothing to reload.
        zendiloos_exec::run(&self.systemctl, &["try-reload-or-restart", "nginx.service"], LIMITS)
            .await?
            .success(&self.systemctl)?;
        Ok(Response::Done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_protocols_nginx_accepts() {
        assert!(tls_config(TlsMinimum::Tls12).ends_with("ssl_protocols TLSv1.2 TLSv1.3;\n"));
        assert!(tls_config(TlsMinimum::Tls13).ends_with("ssl_protocols TLSv1.3;\n"));
    }
}
