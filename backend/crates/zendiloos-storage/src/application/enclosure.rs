//! Where each disk sits (docs/architecture.md, 5): the enclosure chosen,
//! which disk is in which slot, and making a disk's activity light blink
//! so it can be found in its slot.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, RequestContext, Timestamp};

use super::StorageError;
use super::monitor::StorageMonitor;
use super::ports::{
    BlockDeviceSource, DiskLocator, EnclosureCatalog, EnclosureStore, MachineSource, ZfsSource,
};
use super::queries::inventory;
use crate::domain::{self, Disk, EnclosureChoice, EnclosureModel, Health, Layout, Machine, Placement};

/// What zendiloOS knows about the machine's enclosure.
#[derive(Debug, Clone, PartialEq)]
pub struct EnclosureView {
    pub machine: Machine,
    /// The catalog's model for this machine, if it has one.
    pub detected: Option<EnclosureModel>,
    /// `None`: not set up yet.
    pub chosen: Option<ChosenEnclosure>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChosenEnclosure {
    /// The catalog's model; `None` for a layout of one's own.
    pub model: Option<EnclosureModel>,
    /// The model's slots bound otherwise by the administrator.
    pub overridden: Vec<String>,
    /// With the overrides applied.
    pub layout: Layout,
    pub placement: Placement,
    /// The disks present, by kernel name.
    pub disks: Vec<Disk>,
    /// The SMART verdicts of the disks in slots, as the monitor last read
    /// them, by kernel name; a disk not read yet has none.
    pub health: HashMap<String, Health>,
}

/// Choosing the enclosure, and which disk is in which slot.
#[derive(Clone)]
pub struct Enclosures {
    devices: Arc<dyn BlockDeviceSource>,
    zfs: Arc<dyn ZfsSource>,
    machine: Arc<dyn MachineSource>,
    catalog: Arc<dyn EnclosureCatalog>,
    store: Arc<dyn EnclosureStore>,
    monitor: Arc<StorageMonitor>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
}

impl Enclosures {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        zfs: Arc<dyn ZfsSource>,
        machine: Arc<dyn MachineSource>,
        catalog: Arc<dyn EnclosureCatalog>,
        store: Arc<dyn EnclosureStore>,
        monitor: Arc<StorageMonitor>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { devices, zfs, machine, catalog, store, monitor, audit, clock }
    }

    /// The models of the built-in catalog.
    pub fn models(&self) -> Vec<EnclosureModel> {
        self.catalog.models()
    }

    pub async fn view(&self, context: &RequestContext) -> Result<EnclosureView, StorageError> {
        let models = self.catalog.models();
        // Without its names the machine is no model; the rest still works.
        let machine = self.machine.machine(context).await.unwrap_or_else(|error| {
            tracing::warn!(%error, "cannot read the machine's DMI names");
            Machine::default()
        });
        let detected = domain::detect(&models, &machine).cloned();
        let chosen = match self.store.chosen().await? {
            None => None,
            Some(choice) => match choice.layout(&models) {
                Ok(layout) => Some(self.chosen(choice, layout, &models, context).await?),
                // A model a later release dropped, or overrides it no
                // longer takes: as if nothing were chosen.
                Err(error) => {
                    tracing::warn!(%error, "the chosen enclosure no longer fits the catalog");
                    None
                }
            },
        };
        Ok(EnclosureView { machine, detected, chosen })
    }

    async fn chosen(
        &self,
        choice: EnclosureChoice,
        layout: Layout,
        models: &[EnclosureModel],
        context: &RequestContext,
    ) -> Result<ChosenEnclosure, StorageError> {
        let (devices, pools) = inventory(self.devices.as_ref(), self.zfs.as_ref(), context).await?;
        let disks = domain::disks(&devices, &pools);
        let placement = domain::place(&layout, &disks);
        let verdicts = self.monitor.disk_health().await;
        let health = disks
            .iter()
            .filter(|disk| placement.slot_of(&disk.name).is_some())
            .filter_map(|disk| {
                let key = disk.serial.as_deref().unwrap_or(&disk.name);
                verdicts.get(key).map(|health| (disk.name.clone(), *health))
            })
            .collect();
        let (model, overridden) = match choice {
            EnclosureChoice::Model { model, overrides } => (
                models.iter().find(|m| m.id == model).cloned(),
                overrides.into_iter().map(|o| o.slot).collect(),
            ),
            EnclosureChoice::Custom(_) => (None, Vec::new()),
        };
        Ok(ChosenEnclosure { model, overridden, layout, placement, disks, health })
    }

    /// Chooses a model or a layout of one's own; `None` sets nothing up.
    pub async fn choose(
        &self,
        choice: Option<EnclosureChoice>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<EnclosureView, StorageError> {
        if let Some(choice) = &choice {
            choice.layout(&self.catalog.models())?;
        }
        let saved = self.store.save(choice.as_ref()).await;
        let outcome = if saved.is_ok() { Outcome::Success } else { Outcome::Failure };
        let target = match &choice {
            Some(EnclosureChoice::Model { model, .. }) => model.clone(),
            Some(EnclosureChoice::Custom(_)) => "custom".to_owned(),
            None => "none".to_owned(),
        };
        let entry = AuditEntry::new(self.clock.now(), "storage.enclosure_update", outcome, context)
            .by(actor)
            .target(target);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
        saved?;
        self.view(context).await
    }
}

/// How long one read keeps a disk's light on, and the dark time between.
pub const LOCATE_BURST: Duration = Duration::from_millis(700);
pub const LOCATE_PAUSE: Duration = Duration::from_millis(700);
pub const LOCATE_DEFAULT_SECONDS: u16 = 60;
pub const LOCATE_MAX_SECONDS: u16 = 300;

/// A disk blinking until a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocateSession {
    pub disk: String,
    pub until: Timestamp,
}

struct Session {
    id: u64,
    until: Timestamp,
    stop: Arc<Notify>,
}

type Sessions = Arc<Mutex<HashMap<String, Session>>>;

/// Makes disks blink: reads from a disk in bursts with pauses between, one
/// session per disk, until its time is up or it is stopped.
#[derive(Clone)]
pub struct Locator {
    devices: Arc<dyn BlockDeviceSource>,
    host: Arc<dyn DiskLocator>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
    sessions: Sessions,
    next: Arc<AtomicU64>,
}

impl Locator {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        host: Arc<dyn DiskLocator>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self { devices, host, audit, clock, sessions: Sessions::default(), next: Arc::default() }
    }

    /// The disks blinking now, by name.
    pub fn sessions(&self) -> Vec<LocateSession> {
        let sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        let mut list: Vec<LocateSession> = sessions
            .iter()
            .map(|(disk, session)| LocateSession { disk: disk.clone(), until: session.until })
            .collect();
        list.sort_by(|a, b| a.disk.cmp(&b.disk));
        list
    }

    /// Has a present disk blink for `seconds` (60 if not given); a disk
    /// blinking already starts over.
    pub async fn start(
        &self,
        disk: &str,
        seconds: Option<u16>,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<LocateSession, StorageError> {
        let seconds = seconds.unwrap_or(LOCATE_DEFAULT_SECONDS);
        if !(1..=LOCATE_MAX_SECONDS).contains(&seconds) {
            return Err(StorageError::LocateTime);
        }
        let disks = domain::disks(&self.devices.block_devices(context).await?, &[]);
        let disk = disks.iter().find(|d| d.name == disk).ok_or(StorageError::DiskNotFound)?.name.clone();
        let length = Duration::from_secs(u64::from(seconds));
        let until = self.clock.now().saturating_add(length);
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(Notify::new());
        let previous = self
            .sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(disk.clone(), Session { id, until, stop: Arc::clone(&stop) });
        if let Some(previous) = previous {
            previous.stop.notify_one();
        }
        tokio::spawn(blink(
            Arc::clone(&self.host),
            Arc::clone(&self.sessions),
            disk.clone(),
            id,
            stop,
            length,
            context.clone(),
        ));
        self.record("storage.locate", &disk, actor, context).await;
        Ok(LocateSession { disk, until })
    }

    /// Stops a disk blinking; a read under way still ends.
    pub async fn stop(
        &self,
        disk: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), StorageError> {
        let session = self.sessions.lock().unwrap_or_else(PoisonError::into_inner).remove(disk);
        let session = session.ok_or(StorageError::NotLocating)?;
        session.stop.notify_one();
        self.record("storage.locate_stop", disk, actor, context).await;
        Ok(())
    }

    async fn record(&self, action: &'static str, disk: &str, actor: &Actor, context: &RequestContext) {
        let entry =
            AuditEntry::new(self.clock.now(), action, Outcome::Success, context).by(actor).target(disk);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, "cannot write the audit log");
        }
    }
}

/// One session: a burst, a pause, … until `length` is over or `stop`.
async fn blink(
    host: Arc<dyn DiskLocator>,
    sessions: Sessions,
    disk: String,
    id: u64,
    stop: Arc<Notify>,
    length: Duration,
    context: RequestContext,
) {
    let end = tokio::time::Instant::now() + length;
    loop {
        if let Err(error) = host.read_burst(&disk, LOCATE_BURST, &context).await {
            tracing::warn!(%error, disk, "cannot read from the disk to make it blink");
            break;
        }
        let pause = (tokio::time::Instant::now() + LOCATE_PAUSE).min(end);
        tokio::select! {
            () = stop.notified() => break,
            () = tokio::time::sleep_until(pause) => {}
        }
        if tokio::time::Instant::now() >= end {
            break;
        }
    }
    let mut sessions = sessions.lock().unwrap_or_else(PoisonError::into_inner);
    if sessions.get(&disk).is_some_and(|session| session.id == id) {
        sessions.remove(&disk);
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_kernel::Timestamp;

    use super::*;
    use crate::domain::{Binding, InvalidEnclosure, Override, SlotState};
    use crate::testing::{TEST_MODEL, use_cases as testing_ports};

    fn admin() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    fn model_choice(overrides: Vec<Override>) -> EnclosureChoice {
        EnclosureChoice::Model { model: TEST_MODEL.into(), overrides }
    }

    #[tokio::test]
    async fn the_detected_model_is_offered_until_one_is_chosen() {
        let (use_cases, fake, audit) = testing_ports();
        let enclosures = &use_cases.enclosure;
        let view = enclosures.view(&RequestContext::default()).await.unwrap();
        assert_eq!(view.machine.vendor, "QEMU");
        assert!(view.detected.is_none());
        assert!(view.chosen.is_none());

        fake.set_machine("TEST", "NAS 4");
        let view = enclosures.view(&RequestContext::default()).await.unwrap();
        assert_eq!(view.detected.unwrap().id, TEST_MODEL);
        assert!(view.chosen.is_none(), "offered, not chosen");

        let view = enclosures
            .choose(Some(model_choice(vec![])), &admin(), &RequestContext::default())
            .await
            .unwrap();
        let chosen = view.chosen.unwrap();
        assert_eq!(chosen.model.unwrap().id, TEST_MODEL);
        let placed: Vec<_> =
            chosen.placement.slots.iter().map(|p| (p.slot.as_str(), p.state, p.disk.as_deref())).collect();
        assert_eq!(
            placed,
            [
                ("bay1", SlotState::Occupied, Some("sda")),
                ("bay2", SlotState::Occupied, Some("sdb")),
                ("bay3", SlotState::Occupied, Some("sdc")),
                ("bay4", SlotState::Occupied, Some("sdd")),
                ("m2-1", SlotState::Occupied, Some("nvme0n1")),
                ("m2-2", SlotState::Empty, None),
            ]
        );
        assert_eq!(chosen.placement.unplaced, ["sde"]);
        assert!(chosen.health.is_empty(), "the monitor has not looked yet");
        assert_eq!(audit.entries().last().unwrap().action, "storage.enclosure_update");
    }

    #[tokio::test]
    async fn overrides_and_own_layouts_replace_the_model_s_bindings() {
        let (use_cases, fake, _) = testing_ports();
        let enclosures = &use_cases.enclosure;
        let request = RequestContext::default();
        let overrides = vec![
            Override { slot: "bay1".into(), binding: None },
            Override { slot: "m2-2".into(), binding: Some(Binding::Serial("T5".into())) },
        ];
        let view = enclosures.choose(Some(model_choice(overrides)), &admin(), &request).await.unwrap();
        let chosen = view.chosen.unwrap();
        assert_eq!(chosen.overridden, ["bay1", "m2-2"]);
        assert_eq!(chosen.placement.slot_of("sde"), Some("m2-2"));
        assert_eq!(chosen.placement.slot_of("sda"), None);

        let unknown = model_choice(vec![Override { slot: "bay9".into(), binding: None }]);
        let refused = enclosures.choose(Some(unknown), &admin(), &request).await.unwrap_err();
        assert!(matches!(refused, StorageError::InvalidEnclosure(InvalidEnclosure::UnknownSlot(_))));
        let gone = EnclosureChoice::Model { model: "acme-99".into(), overrides: vec![] };
        assert!(matches!(
            enclosures.choose(Some(gone.clone()), &admin(), &request).await,
            Err(StorageError::InvalidEnclosure(InvalidEnclosure::UnknownModel(_)))
        ));

        let own = crate::testing::test_catalog()[0].layout.clone();
        let view = enclosures.choose(Some(EnclosureChoice::Custom(own)), &admin(), &request).await.unwrap();
        let chosen = view.chosen.unwrap();
        assert!(chosen.model.is_none());
        assert!(chosen.overridden.is_empty());
        assert_eq!(chosen.placement.slot_of("sdb"), Some("bay2"));

        // A model dropped from the catalog since: as if nothing were chosen.
        fake.choose_directly(gone);
        assert!(enclosures.view(&request).await.unwrap().chosen.is_none());
        let view = enclosures.choose(None, &admin(), &request).await.unwrap();
        assert!(view.chosen.is_none());
    }

    #[tokio::test]
    async fn the_monitor_s_verdicts_colour_the_slots() {
        let (use_cases, _, _) = testing_ports();
        use_cases.monitor.check().await;
        let view = use_cases
            .enclosure
            .choose(Some(model_choice(vec![])), &admin(), &RequestContext::default())
            .await
            .unwrap();
        let health = view.chosen.unwrap().health;
        // sdb sleeps and was not read; nvme0n1 is the system disk, not
        // read either.
        assert_eq!(health.get("sda"), Some(&Health::Good));
        assert!(!health.contains_key("sdb"));
        assert!(!health.contains_key("sde"), "not in a slot");
    }

    #[tokio::test]
    async fn a_machine_that_cannot_be_read_is_no_model() {
        let (use_cases, fake, _) = testing_ports();
        fake.set_machine("TEST", "NAS 4");
        *fake.machine_fails.lock().unwrap() = true;
        let view = use_cases.enclosure.view(&RequestContext::default()).await.unwrap();
        assert_eq!(view.machine, Machine::default());
        assert!(view.detected.is_none());
        assert_eq!(use_cases.enclosure.models()[0].id, TEST_MODEL);
    }

    #[tokio::test(start_paused = true)]
    async fn a_disk_blinks_in_bursts_until_its_time_is_up() {
        let (use_cases, fake, audit) = testing_ports();
        let locate = &use_cases.locate;
        let request = RequestContext::default();
        let session = locate.start("sdc", Some(3), &admin(), &request).await.unwrap();
        assert_eq!(
            session,
            LocateSession { disk: "sdc".into(), until: Timestamp::from_unix_seconds(1_793_880_003) }
        );
        assert_eq!(locate.sessions(), [session]);
        assert_eq!(audit.entries().last().unwrap().action, "storage.locate");

        tokio::time::sleep(Duration::from_secs(10)).await;
        assert!(locate.sessions().is_empty(), "over after three seconds");
        // A burst every 1.4 s (the fake's reads take 0.7 s): at 0, 1.4 and 2.8 s.
        assert_eq!(fake.bursts(), ["sdc", "sdc", "sdc"]);
    }

    #[tokio::test(start_paused = true)]
    async fn blinking_stops_on_request_and_starts_over_when_asked_again() {
        let (use_cases, fake, audit) = testing_ports();
        let locate = &use_cases.locate;
        let request = RequestContext::default();
        locate.start("sda", None, &admin(), &request).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let again = locate.start("sda", Some(300), &admin(), &request).await.unwrap();
        assert_eq!(again.until, Timestamp::from_unix_seconds(1_793_880_300));
        assert_eq!(locate.sessions().len(), 1, "one session per disk");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(locate.sessions(), [again], "the first one's end does not end the second");

        locate.stop("sda", &admin(), &request).await.unwrap();
        assert!(locate.sessions().is_empty());
        assert_eq!(audit.entries().last().unwrap().action, "storage.locate_stop");
        let bursts = fake.bursts().len();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(fake.bursts().len() <= bursts + 1, "at most the read under way");
        assert!(matches!(locate.stop("sda", &admin(), &request).await, Err(StorageError::NotLocating)));
    }

    #[tokio::test(start_paused = true)]
    async fn only_present_disks_blink_for_a_bounded_time() {
        let (use_cases, fake, _) = testing_ports();
        let locate = &use_cases.locate;
        let request = RequestContext::default();
        for seconds in [0, LOCATE_MAX_SECONDS + 1] {
            assert!(matches!(
                locate.start("sda", Some(seconds), &admin(), &request).await,
                Err(StorageError::LocateTime)
            ));
        }
        assert!(matches!(
            locate.start("sdz", None, &admin(), &request).await,
            Err(StorageError::DiskNotFound)
        ));

        // A disk that cannot be read ends its session.
        *fake.burst_fails.lock().unwrap() = true;
        locate.start("sdb", None, &admin(), &request).await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(locate.sessions().is_empty());
    }
}
