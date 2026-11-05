use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::application::ports::PowerControl;
use crate::domain::PowerAction;

/// Command: reboots or powers off the system. The caller has checked the
/// permission.
#[derive(Clone)]
pub struct RequestPowerAction {
    power: Arc<dyn PowerControl>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl RequestPowerAction {
    pub fn new(power: Arc<dyn PowerControl>, audit: Arc<dyn AuditLog>, clock: Arc<dyn Clock>) -> Self {
        Self { power, audit, clock }
    }

    pub async fn execute(
        &self,
        action: PowerAction,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<(), PortError> {
        let entry =
            |outcome| AuditEntry::new(self.clock.now(), action.audit_action(), outcome, request).by(actor);
        // Recorded first: once the system goes down, there may be no
        // chance to write it afterwards.
        self.audit.record(entry(Outcome::Success)).await?;
        if let Err(error) = self.power.request(action, request).await {
            self.audit.record(entry(Outcome::Failure).reason("agent_failed")).await?;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;

    use super::*;

    #[derive(Default)]
    struct Recorder {
        actions: Mutex<Vec<PowerAction>>,
        entries: Mutex<Vec<AuditEntry>>,
        fail: bool,
    }

    #[async_trait]
    impl PowerControl for Recorder {
        async fn request(&self, action: PowerAction, _: &RequestContext) -> Result<(), PortError> {
            if self.fail {
                return Err(PortError::new("agent", "down"));
            }
            self.actions.lock().unwrap().push(action);
            Ok(())
        }
    }

    #[async_trait]
    impl AuditLog for Recorder {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.entries.lock().unwrap().push(entry);
            Ok(())
        }
    }

    impl Clock for Recorder {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_seconds(100)
        }
    }

    fn command(recorder: &Arc<Recorder>) -> RequestPowerAction {
        RequestPowerAction::new(recorder.clone(), recorder.clone(), recorder.clone())
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".to_owned() }
    }

    #[tokio::test]
    async fn records_who_rebooted_before_rebooting() {
        let recorder = Arc::new(Recorder::default());

        command(&recorder).execute(PowerAction::Reboot, &alice(), &RequestContext::default()).await.unwrap();

        assert_eq!(*recorder.actions.lock().unwrap(), [PowerAction::Reboot]);
        let entries = recorder.entries.lock().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].action, entries[0].outcome), ("system.reboot", Outcome::Success));
        assert_eq!(entries[0].actor.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn records_a_failed_request() {
        let recorder = Arc::new(Recorder { fail: true, ..Recorder::default() });

        let result =
            command(&recorder).execute(PowerAction::PowerOff, &alice(), &RequestContext::default()).await;

        assert!(result.is_err());
        let entries = recorder.entries.lock().unwrap();
        let outcomes: Vec<_> = entries.iter().map(|e| (e.outcome, e.reason)).collect();
        assert_eq!(outcomes, [(Outcome::Success, None), (Outcome::Failure, Some("agent_failed"))]);
    }
}
