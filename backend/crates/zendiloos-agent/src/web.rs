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

    /// nginx's settings in `dir`, with systemctl a script that fails while
    /// `fail` is there.
    fn web(dir: &std::path::Path) -> Web {
        let systemctl = dir.join("systemctl");
        fs::write(
            &systemctl,
            format!("#!/bin/sh\n[ -f {} ] && exit 1\nexit 0\n", dir.join("fail").display()),
        )
        .unwrap();
        fs::set_permissions(&systemctl, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let tools = Tools { systemctl, ..Tools::default() };
        Web::new(&tools, WebPaths { tls: dir.join("nginx/tls.conf") }).unwrap()
    }

    #[tokio::test]
    async fn the_oldest_version_is_written_and_nginx_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let web = web(dir.path());
        assert_eq!(web.state().unwrap(), Response::WebTlsState { config: String::new() }, "none yet");
        assert_eq!(web.set_tls(TlsMinimum::Tls13).await.unwrap(), Response::Done);
        let Response::WebTlsState { config } = web.state().unwrap() else { panic!() };
        assert!(config.contains("TLSv1.3;"));

        fs::write(dir.path().join("fail"), "").unwrap();
        assert!(web.set_tls(TlsMinimum::Tls12).await.is_err(), "nginx did not take it");
        fs::remove_dir_all(dir.path().join("nginx")).unwrap();
        fs::write(dir.path().join("nginx"), "a file").unwrap();
        assert!(web.set_tls(TlsMinimum::Tls12).await.is_err(), "no folder for it");
        fs::remove_file(dir.path().join("nginx")).unwrap();
        fs::create_dir_all(dir.path().join("nginx/tls.conf/inside")).unwrap();
        assert!(web.state().is_err(), "not readable");
    }
}
