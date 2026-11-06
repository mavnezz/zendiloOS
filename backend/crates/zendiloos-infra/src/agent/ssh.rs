use std::sync::Arc;

use async_trait::async_trait;
use zendiloos_agent_proto::{self as proto, AgentTransport, Request, Response};
use zendiloos_kernel::{PortError, RequestContext, Timestamp};
use zendiloos_system::application::ports::SshControl;
use zendiloos_system::domain::{RootLogin, SshSettings, SshStatus};

use super::{WHAT, call, expect_done, unexpected};

/// [`SshControl`] through the system agent.
pub struct AgentSsh {
    transport: Arc<dyn AgentTransport>,
}

impl AgentSsh {
    pub fn new(transport: Arc<dyn AgentTransport>) -> Self {
        Self { transport }
    }
}

fn invalid(what: &str) -> PortError {
    PortError::new(WHAT, format!("not a valid {what} for the agent"))
}

/// The value of `keyword` in the settings file zendiloOS wrote.
fn setting<'a>(config: &'a str, keyword: &str) -> Option<&'a str> {
    config.lines().find_map(|line| line.strip_prefix(keyword)?.strip_prefix(' ')).map(str::trim)
}

fn status(installed: bool, enabled: &str, active: &str, config: &str, off_at: Option<i64>) -> SshStatus {
    SshStatus {
        installed,
        // `enabled-runtime` and `alias` start it too.
        enabled: enabled.starts_with("enabled") || enabled == "alias",
        running: active == "active" || active == "reloading",
        port: setting(config, "Port").and_then(|port| port.parse().ok()).unwrap_or(22),
        root_login: match setting(config, "PermitRootLogin") {
            Some("no") => Some(RootLogin::No),
            Some("prohibit-password" | "without-password") => Some(RootLogin::KeyOnly),
            Some("yes") => Some(RootLogin::Yes),
            _ => None,
        },
        password_login: match setting(config, "PasswordAuthentication") {
            Some("yes") => Some(true),
            Some("no") => Some(false),
            _ => None,
        },
        off_at: off_at.map(Timestamp::from_unix_seconds),
    }
}

#[async_trait]
impl SshControl for AgentSsh {
    async fn status(&self, context: &RequestContext) -> Result<SshStatus, PortError> {
        match call(self.transport.as_ref(), Request::SshState {}, context).await? {
            Response::SshState { installed, enabled, active, config, off_at } => {
                Ok(status(installed, &enabled, &active, &config, off_at))
            }
            other => Err(unexpected(other)),
        }
    }

    async fn apply(&self, settings: &SshSettings, context: &RequestContext) -> Result<(), PortError> {
        let config = proto::SshConfig {
            enabled: settings.enabled,
            port: proto::SshPort::new(settings.port).map_err(|_| invalid("port"))?,
            root_login: settings.root_login.map(|root| match root {
                RootLogin::No => proto::RootLogin::No,
                RootLogin::KeyOnly => proto::RootLogin::KeyOnly,
                RootLogin::Yes => proto::RootLogin::Yes,
            }),
            password_login: settings.password_login,
            off_after: settings
                .off_after_minutes
                .map(|minutes| proto::OffAfter::minutes(minutes).map_err(|_| invalid("time to turn off")))
                .transpose()?,
        };
        expect_done(self.transport.as_ref(), Request::SetSsh { config }, context).await
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_sim::SimulatedAgent;

    use super::*;

    #[test]
    fn reads_what_the_agent_found() {
        let found = status(
            true,
            "enabled",
            "active",
            "# zendiloOS\nPort 2222\nPermitRootLogin prohibit-password\n",
            Some(5),
        );
        assert!(found.enabled && found.running);
        assert_eq!(
            (found.port, found.root_login, found.password_login),
            (2222, Some(RootLogin::KeyOnly), None)
        );
        assert_eq!(found.off_at, Some(Timestamp::from_unix_seconds(5)));
        let plain = status(false, "disabled", "inactive", "", None);
        assert!(!plain.installed && !plain.enabled && !plain.running);
        assert_eq!(plain.port, 22, "sshd's default");
    }

    #[tokio::test]
    async fn settings_go_through_the_agent() {
        let ssh = AgentSsh::new(Arc::new(SimulatedAgent::new()));
        let context = RequestContext::default();
        let settings = SshSettings {
            enabled: true,
            port: 2222,
            root_login: Some(RootLogin::No),
            password_login: Some(false),
            off_after_minutes: Some(10),
        };

        ssh.apply(&settings, &context).await.unwrap();

        let found = ssh.status(&context).await.unwrap();
        assert!(found.running && found.off_at.is_some());
        assert_eq!(
            (found.port, found.root_login, found.password_login),
            (2222, Some(RootLogin::No), Some(false))
        );
    }
}
