use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::application::ports::{HostNameControl, HostNameObserver};
use crate::domain::{HostName, InvalidHostName, Renamed};

#[derive(Debug, thiserror::Error)]
pub enum SetHostNameError {
    #[error(transparent)]
    Invalid(#[from] InvalidHostName),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Command: renames the machine. What derives from the name follows on the
/// host without a restart (the agent sees to it); within zendiloOS, the
/// notifications' subject. The caller has checked the permission. The same
/// name again is applied again, which repairs what did not follow it.
#[derive(Clone)]
pub struct SetHostName {
    host_name: Arc<dyn HostNameControl>,
    observer: Arc<dyn HostNameObserver>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl SetHostName {
    pub fn new(
        host_name: Arc<dyn HostNameControl>,
        observer: Arc<dyn HostNameObserver>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { host_name, observer, audit, clock }
    }

    pub async fn execute(
        &self,
        name: &str,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<Renamed, SetHostNameError> {
        let name = HostName::parse(name)?;
        let renamed = self.host_name.rename(&name, request).await;
        let outcome = if renamed.is_ok() { Outcome::Success } else { Outcome::Failure };
        let entry = AuditEntry::new(self.clock.now(), "system.host_name_update", outcome, request)
            .by(actor)
            .target(name.as_str());
        self.audit.record(entry).await?;
        let renamed = renamed?;
        self.observer.renamed(&renamed.name);
        Ok(renamed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;

    use super::*;

    #[derive(Default)]
    struct FakeHost {
        names: Mutex<Vec<String>>,
        fail: bool,
    }

    #[async_trait]
    impl HostNameControl for FakeHost {
        async fn name(&self, _: &RequestContext) -> Result<String, PortError> {
            Ok(self.names.lock().unwrap().last().cloned().unwrap_or_else(|| "zendiloos".into()))
        }

        async fn rename(&self, name: &HostName, _: &RequestContext) -> Result<Renamed, PortError> {
            if self.fail {
                return Err(PortError::new("agent", "Invalid hostname"));
            }
            self.names.lock().unwrap().push(name.as_str().to_owned());
            Ok(Renamed {
                name: name.as_str().to_owned(),
                certificate_renewed: true,
                own_certificate_served: true,
            })
        }
    }

    #[derive(Default)]
    struct Followers(Mutex<Vec<String>>);

    impl HostNameObserver for Followers {
        fn renamed(&self, name: &str) {
            self.0.lock().unwrap().push(name.to_owned());
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

    fn command(fail: bool) -> (SetHostName, Arc<FakeHost>, Arc<Followers>, Arc<Audit>) {
        let host = Arc::new(FakeHost { fail, ..FakeHost::default() });
        let (followers, audit) = (Arc::new(Followers::default()), Arc::new(Audit::default()));
        (
            SetHostName::new(host.clone(), followers.clone(), audit.clone(), Arc::new(Fixed)),
            host,
            followers,
            audit,
        )
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".to_owned() }
    }

    #[tokio::test]
    async fn renames_tidied_tells_who_follows_and_records_it() {
        let (command, host, followers, audit) = command(false);

        let renamed = command.execute(" Office ", &alice(), &RequestContext::default()).await.unwrap();

        assert_eq!(renamed.name, "office");
        assert_eq!(host.names.lock().unwrap()[..], ["office"]);
        assert_eq!(followers.0.lock().unwrap()[..], ["office"]);
        let entries = audit.0.lock().unwrap();
        assert_eq!(entries[0].action, "system.host_name_update");
        assert_eq!(entries[0].target.as_deref(), Some("office"));
        assert_eq!(entries[0].outcome, Outcome::Success);
    }

    #[tokio::test]
    async fn an_invalid_name_is_refused_before_anything_changes() {
        let (command, host, followers, audit) = command(false);

        let result = command.execute("nas.local", &alice(), &RequestContext::default()).await;

        assert!(matches!(result, Err(SetHostNameError::Invalid(InvalidHostName::Characters))));
        assert!(host.names.lock().unwrap().is_empty() && followers.0.lock().unwrap().is_empty());
        assert!(audit.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failure_is_recorded_and_nobody_follows() {
        let (command, _, followers, audit) = command(true);

        let result = command.execute("office", &alice(), &RequestContext::default()).await;

        assert!(matches!(result, Err(SetHostNameError::Port(_))));
        assert_eq!(audit.0.lock().unwrap()[0].outcome, Outcome::Failure);
        assert!(followers.0.lock().unwrap().is_empty());
    }
}
