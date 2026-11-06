use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::application::ports::{ListeningPorts, SshControl};
use crate::domain::{InvalidSshSettings, SshSettings, SshStatus};

#[derive(Debug, thiserror::Error)]
pub enum SetSshError {
    #[error(transparent)]
    Invalid(#[from] InvalidSshSettings),
    /// There is no SSH server to turn on.
    #[error("the SSH server is not installed")]
    NotInstalled,
    /// Another program listens on the port.
    #[error("port {0} is in use")]
    PortInUse(u16),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Command: turns the SSH server on or off and sets its port and rules.
/// The caller has checked the permission.
#[derive(Clone)]
pub struct SetSsh {
    ssh: Arc<dyn SshControl>,
    listening: Arc<dyn ListeningPorts>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl SetSsh {
    pub fn new(
        ssh: Arc<dyn SshControl>,
        listening: Arc<dyn ListeningPorts>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { ssh, listening, audit, clock }
    }

    /// The server afterwards.
    pub async fn execute(
        &self,
        settings: SshSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<SshStatus, SetSshError> {
        let settings = settings.checked()?;
        let current = self.ssh.status(request).await?;
        if !current.installed {
            return Err(SetSshError::NotInstalled);
        }
        // sshd could not take the port and would stop; its own does not count.
        let own = current.running && current.port == settings.port;
        if settings.enabled && !own && self.listening.is_listening(settings.port).await? {
            return Err(SetSshError::PortInUse(settings.port));
        }
        let applied = self.ssh.apply(&settings, request).await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        let target = if settings.enabled { format!("on, port {}", settings.port) } else { "off".to_owned() };
        let entry =
            AuditEntry::new(self.clock.now(), "system.ssh_update", outcome, request).by(actor).target(target);
        self.audit.record(entry).await?;
        applied?;
        Ok(self.ssh.status(request).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;

    use super::*;

    struct FakeSsh {
        installed: bool,
        applied: Mutex<Vec<SshSettings>>,
    }

    #[async_trait]
    impl SshControl for FakeSsh {
        async fn status(&self, _: &RequestContext) -> Result<SshStatus, PortError> {
            let last = self.applied.lock().unwrap().last().copied();
            Ok(SshStatus {
                installed: self.installed,
                enabled: last.is_some_and(|s| s.enabled),
                running: last.is_some_and(|s| s.enabled),
                port: last.map_or(22, |s| s.port),
                root_login: None,
                password_login: None,
                off_at: None,
            })
        }

        async fn apply(&self, settings: &SshSettings, _: &RequestContext) -> Result<(), PortError> {
            self.applied.lock().unwrap().push(*settings);
            Ok(())
        }
    }

    /// Something listens on 8080.
    struct Listening;

    #[async_trait]
    impl ListeningPorts for Listening {
        async fn is_listening(&self, port: u16) -> Result<bool, PortError> {
            Ok(port == 8080)
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<AuditEntry>>);

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.0.lock().unwrap().push(entry);
            Ok(())
        }
    }

    struct Fixed;

    impl Clock for Fixed {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(1_000)
        }
    }

    fn command(installed: bool) -> (SetSsh, Arc<FakeSsh>, Arc<Audit>) {
        let ssh = Arc::new(FakeSsh { installed, applied: Mutex::default() });
        let audit = Arc::new(Audit::default());
        (SetSsh::new(ssh.clone(), Arc::new(Listening), audit.clone(), Arc::new(Fixed)), ssh, audit)
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".to_owned() }
    }

    fn settings(port: u16) -> SshSettings {
        SshSettings {
            enabled: true,
            port,
            root_login: None,
            password_login: None,
            off_after_minutes: Some(10),
        }
    }

    #[tokio::test]
    async fn applies_and_records_it() {
        let (command, ssh, audit) = command(true);

        let status = command.execute(settings(2222), &alice(), &RequestContext::default()).await.unwrap();

        assert!(status.running && status.port == 2222);
        assert_eq!(ssh.applied.lock().unwrap().len(), 1);
        let entries = audit.0.lock().unwrap();
        assert_eq!(
            (entries[0].action, entries[0].target.as_deref()),
            ("system.ssh_update", Some("on, port 2222"))
        );
    }

    #[tokio::test]
    async fn nothing_is_turned_on_that_is_not_there_or_on_a_wrong_port() {
        let (command, ssh, audit) = command(false);
        let missing = command.execute(settings(22), &alice(), &RequestContext::default()).await;
        assert!(matches!(missing, Err(SetSshError::NotInstalled)));
        let wrong = command.execute(settings(445), &alice(), &RequestContext::default()).await;
        assert!(matches!(wrong, Err(SetSshError::Invalid(InvalidSshSettings::Port))));
        assert!(ssh.applied.lock().unwrap().is_empty() && audit.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_port_another_program_listens_on_is_refused() {
        let (command, ssh, _) = command(true);
        let taken = command.execute(settings(8080), &alice(), &RequestContext::default()).await;
        assert!(matches!(taken, Err(SetSshError::PortInUse(8080))));
        assert!(ssh.applied.lock().unwrap().is_empty());
        let off = SshSettings { enabled: false, ..settings(8080) };
        assert!(
            command.execute(off, &alice(), &RequestContext::default()).await.is_ok(),
            "off, it takes none"
        );
    }
}
