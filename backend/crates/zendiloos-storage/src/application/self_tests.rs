//! Self-tests on a schedule (docs/architecture.md, 5): the plans, and
//! starting the tests whose time has come.

use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext};

use super::StorageError;
use super::ports::{BlockDeviceSource, SelfTestPlanStore, SmartSource};
use crate::domain::{self, DeviceKind, ScheduledSelfTest, SelfTestPlan, SmartReport};

/// The plans, and the host's offset from UTC that their hours are in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfTestPlans {
    pub plans: Vec<ScheduledSelfTest>,
    /// Seconds the host's clock is ahead of UTC now.
    pub utc_offset: i32,
}

#[derive(Clone)]
pub struct SelfTestSchedule {
    devices: Arc<dyn BlockDeviceSource>,
    smart: Arc<dyn SmartSource>,
    store: Arc<dyn SelfTestPlanStore>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl SelfTestSchedule {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        smart: Arc<dyn SmartSource>,
        store: Arc<dyn SelfTestPlanStore>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { devices, smart, store, audit, clock }
    }

    pub async fn plans(&self) -> Result<SelfTestPlans, StorageError> {
        let plans = self.store.plans().await?;
        Ok(SelfTestPlans { plans, utc_offset: self.clock.utc_offset(self.clock.now()) })
    }

    /// Replaces every plan; no plan for a kind means no such tests.
    pub async fn set_plans(
        &self,
        plans: Vec<SelfTestPlan>,
        actor: &Actor,
        request: &RequestContext,
    ) -> Result<SelfTestPlans, StorageError> {
        let plans = domain::self_test_plans(plans)?;
        let saved = self.store.save(&plans).await;
        let outcome = if saved.is_ok() { Outcome::Success } else { Outcome::Failure };
        let entry = AuditEntry::new(self.clock.now(), "storage.self_test_plans", outcome, request).by(actor);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
        saved?;
        self.plans().await
    }

    /// Starts the tests of the plans that are due; call every few minutes.
    /// A disk that sleeps is woken: the plan asked for it. A failing test
    /// shows in the disk's SMART, which the monitor reports.
    pub async fn run_due(&self) {
        let request = RequestContext::default();
        let now = self.clock.now();
        let offset = self.clock.utc_offset(now);
        let plans = match self.store.plans().await {
            Ok(plans) => plans,
            Err(error) => return tracing::warn!(%error, "self-tests: cannot read the plans"),
        };
        // The long test first: it covers what the short one checks, and a
        // drive runs one test at a time.
        let due: Vec<ScheduledSelfTest> =
            plans.into_iter().rev().filter(|s| s.plan.is_due(s.last_run, now, offset)).collect();
        if due.is_empty() {
            return;
        }
        let disks = match self.devices.block_devices(&request).await {
            Ok(devices) => devices.into_iter().filter(|d| d.kind == DeviceKind::Disk).collect::<Vec<_>>(),
            Err(error) => return tracing::warn!(%error, "self-tests: cannot list the disks"),
        };
        let mut started: Vec<&str> = Vec::new();
        for ScheduledSelfTest { plan, .. } in &due {
            let kind = plan.kind.as_str();
            for disk in disks.iter().filter(|d| plan.covers(d.serial.as_deref())) {
                if started.contains(&disk.name.as_str()) {
                    continue;
                }
                match self.smart.smart_report(&disk.name, &request).await {
                    Ok(SmartReport::Available(data))
                        if data.self_tests_supported && data.running_self_test.is_none() => {}
                    Ok(SmartReport::Standby) => {}
                    Ok(_) => continue,
                    Err(error) => {
                        tracing::warn!(%error, disk = disk.name, "self-tests: cannot read SMART");
                        continue;
                    }
                }
                match self.smart.start_self_test(&disk.name, plan.kind, &request).await {
                    Ok(()) => {
                        tracing::info!(disk = disk.name, kind, "self-tests: started on schedule");
                        started.push(&disk.name);
                    }
                    Err(error) => tracing::warn!(%error, disk = disk.name, kind, "self-tests: cannot start"),
                }
            }
            // Recorded even if a disk failed: retrying every five minutes
            // would not make it work, and the next run comes.
            if let Err(error) = self.store.ran(plan.kind, now).await {
                tracing::warn!(%error, kind, "self-tests: cannot record the run");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_jobs::Jobs;
    use zendiloos_jobs::testing::MemoryJobs;
    use zendiloos_kernel::{PortError, Timestamp};

    use super::*;
    use crate::application::StorageUseCases;
    use crate::domain::{SelfTestFrequency, SelfTestKind};
    use crate::testing::FakeStorage;

    const HOUR: i64 = 3600;
    const MIDNIGHT: i64 = 1_793_836_800; // 2026-11-05 00:00:00 UTC

    /// A clock an hour ahead of UTC, as in Berlin in winter.
    struct Berlin(Mutex<Timestamp>);

    impl Berlin {
        fn set(&self, seconds: i64) {
            *self.0.lock().unwrap() = Timestamp::from_unix_seconds(MIDNIGHT + seconds);
        }
    }

    impl Clock for Berlin {
        fn now(&self) -> Timestamp {
            *self.0.lock().unwrap()
        }

        fn utc_offset(&self, _: Timestamp) -> i32 {
            3600
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<&'static str>>);

    #[async_trait]
    impl AuditLog for Audit {
        async fn record(&self, entry: AuditEntry) -> Result<(), PortError> {
            self.0.lock().unwrap().push(entry.action);
            Ok(())
        }
    }

    fn setup() -> (StorageUseCases, Arc<FakeStorage>, Arc<Berlin>, Arc<Audit>) {
        let clock = Arc::new(Berlin(Mutex::new(Timestamp::from_unix_seconds(MIDNIGHT))));
        let audit = Arc::new(Audit::default());
        let jobs = Jobs::new(Arc::new(MemoryJobs::default()), clock.clone());
        let (ports, fake) = FakeStorage::ports(jobs, audit.clone(), clock.clone());
        (StorageUseCases::new(ports), fake, clock, audit)
    }

    fn alice() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    #[tokio::test]
    async fn plans_start_their_tests_in_the_local_window_the_long_one_first() {
        let (storage, fake, clock, audit) = setup();
        let schedule = &storage.self_test_schedule;
        let request = RequestContext::default();
        let plans = vec![
            SelfTestPlan {
                kind: SelfTestKind::Short,
                frequency: SelfTestFrequency::Daily,
                hour: 2,
                disks: vec![],
            },
            SelfTestPlan {
                kind: SelfTestKind::Long,
                frequency: SelfTestFrequency::Weekly,
                hour: 3,
                disks: vec!["T1".into(), "T3".into()],
            },
        ];
        let saved = schedule.set_plans(plans, &alice(), &request).await.unwrap();
        assert_eq!(saved.utc_offset, 3600);
        assert_eq!(saved.plans.len(), 2);
        assert_eq!(*audit.0.lock().unwrap(), ["storage.self_test_plans"]);

        // One at night UTC is two in Berlin: only the short plan is due.
        clock.set(HOUR - 10 * 60);
        schedule.run_due().await;
        assert!(fake.calls().is_empty(), "before the window");

        // Three in Berlin: both. The long test goes to the disks it names;
        // the short one to the others that can test themselves, the
        // sleeping sdb woken, the NVMe disk (it cannot) left out.
        clock.set(2 * HOUR);
        schedule.run_due().await;
        assert_eq!(
            fake.calls(),
            [
                "self-test sda Long",
                "self-test sdc Long",
                "self-test sdb Short",
                "self-test sdd Short",
                "self-test sde Short"
            ]
        );
        let ran = schedule.plans().await.unwrap();
        assert!(ran.plans.iter().all(|s| s.last_run == Some(clock.now())));

        // Not again that night; the next night only the daily one, and a
        // disk still testing is left alone.
        clock.set(3 * HOUR);
        schedule.run_due().await;
        assert_eq!(fake.calls().len(), 5);
        fake.testing.lock().unwrap().retain(|disk| disk != "sdd");
        clock.set(26 * HOUR);
        schedule.run_due().await;
        assert_eq!(&fake.calls()[5..], ["self-test sdb Short", "self-test sdd Short"]);
    }

    #[tokio::test]
    async fn plans_are_checked_before_they_are_kept() {
        let (storage, _, _, audit) = setup();
        let nightly_long = SelfTestPlan {
            kind: SelfTestKind::Long,
            frequency: SelfTestFrequency::Daily,
            hour: 2,
            disks: vec![],
        };

        let refused = storage
            .self_test_schedule
            .set_plans(vec![nightly_long], &alice(), &RequestContext::default())
            .await;

        assert!(matches!(refused, Err(StorageError::InvalidSelfTestPlan(_))), "{refused:?}");
        assert!(storage.self_test_schedule.plans().await.unwrap().plans.is_empty());
        assert!(audit.0.lock().unwrap().is_empty());
    }
}
