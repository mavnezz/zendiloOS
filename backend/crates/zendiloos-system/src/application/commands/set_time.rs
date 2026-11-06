use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};

use crate::application::ports::TimeControl;
use crate::domain::{InvalidTimeSettings, TimeSettings, TimeStatus};

#[derive(Debug, thiserror::Error)]
pub enum SetTimeError {
    #[error(transparent)]
    Invalid(#[from] InvalidTimeSettings),
    #[error(transparent)]
    Port(#[from] PortError),
}

/// Command: sets the time zone, the time servers, and the time when no
/// server keeps it. The caller has checked the permission.
#[derive(Clone)]
pub struct SetTime {
    time: Arc<dyn TimeControl>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl SetTime {
    pub fn new(time: Arc<dyn TimeControl>, audit: Arc<dyn AuditLog>, clock: Arc<dyn Clock>) -> Self {
        Self { time, audit, clock }
    }

    /// The clock afterwards.
    pub async fn execute(
        &self,
        settings: TimeSettings,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<TimeStatus, SetTimeError> {
        let settings = settings.checked()?;
        // The system's own list decides, so the answer is a clear "no such
        // zone" rather than a failure halfway through.
        if !self.time.status(request).await?.zones.contains(&settings.zone) {
            return Err(InvalidTimeSettings::Zone(settings.zone).into());
        }
        let applied = self.time.apply(&settings, request).await;
        let outcome = if applied.is_ok() { Outcome::Success } else { Outcome::Failure };
        // Taken after the change: a clock set by hand stamps its own entry.
        let entry = AuditEntry::new(self.clock.now(), "system.time_update", outcome, request)
            .by(actor)
            .target(&settings.zone);
        self.audit.record(entry).await?;
        applied?;
        Ok(self.time.status(request).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_kernel::Timestamp;

    use super::*;

    struct FakeTime {
        applied: Mutex<Vec<TimeSettings>>,
        fail: bool,
    }

    #[async_trait]
    impl TimeControl for FakeTime {
        async fn status(&self, _: &RequestContext) -> Result<TimeStatus, PortError> {
            let applied = self.applied.lock().unwrap();
            let current = applied.last();
            Ok(TimeStatus {
                zone: current.map_or("Etc/UTC".into(), |s| s.zone.clone()),
                network_time: current.is_none_or(|s| s.network_time),
                can_network_time: true,
                synchronized: true,
                servers: current.map(|s| s.servers.clone()).unwrap_or_default(),
                server_in_use: None,
                zones: vec!["Etc/UTC".into(), "Europe/Berlin".into()],
                now: Timestamp::from_unix_seconds(1_000),
            })
        }

        async fn apply(&self, settings: &TimeSettings, _: &RequestContext) -> Result<(), PortError> {
            if self.fail {
                return Err(PortError::new("agent", "refused"));
            }
            self.applied.lock().unwrap().push(settings.clone());
            Ok(())
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

    fn command(fail: bool) -> (SetTime, Arc<FakeTime>, Arc<Audit>) {
        let time = Arc::new(FakeTime { applied: Mutex::default(), fail });
        let audit = Arc::new(Audit::default());
        (SetTime::new(time.clone(), audit.clone(), Arc::new(Fixed)), time, audit)
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".to_owned() }
    }

    fn settings(zone: &str) -> TimeSettings {
        TimeSettings { zone: zone.into(), network_time: true, servers: vec![" PTB.de ".into()], clock: None }
    }

    #[tokio::test]
    async fn applies_tidied_settings_and_records_it() {
        let (command, time, audit) = command(false);

        let status =
            command.execute(settings("Europe/Berlin"), &alice(), &RequestContext::default()).await.unwrap();

        assert_eq!(status.zone, "Europe/Berlin");
        assert_eq!(time.applied.lock().unwrap()[0].servers, ["ptb.de"]);
        let entries = audit.0.lock().unwrap();
        assert_eq!(entries[0].action, "system.time_update");
        assert_eq!(entries[0].outcome, Outcome::Success);
    }

    #[tokio::test]
    async fn a_zone_the_system_lacks_is_refused_before_anything_changes() {
        let (command, time, audit) = command(false);

        let result = command.execute(settings("Mars/Olympus"), &alice(), &RequestContext::default()).await;

        assert!(matches!(result, Err(SetTimeError::Invalid(InvalidTimeSettings::Zone(_)))));
        assert!(time.applied.lock().unwrap().is_empty());
        assert!(audit.0.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_failure_is_recorded_too() {
        let (command, _, audit) = command(true);

        let result = command.execute(settings("Etc/UTC"), &alice(), &RequestContext::default()).await;

        assert!(matches!(result, Err(SetTimeError::Port(_))));
        assert_eq!(audit.0.lock().unwrap()[0].outcome, Outcome::Failure);
    }
}
