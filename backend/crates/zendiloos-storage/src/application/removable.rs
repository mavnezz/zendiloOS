//! Removable drives: mounted when they are seen, as Windows does, ejected
//! on request, and let go of when pulled out without being ejected.
//!
//! zendiloosd sees a drive when it looks: whenever the drives are listed
//! (the file explorer asks while it is open) and once a minute besides
//! ([`RemovableDrives::watch`]). A drive ejected without being powered
//! off, or one that could not be mounted, stays as it is until it is
//! pulled out or mounted by hand.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use zendiloos_kernel::{Actor, AuditEntry, AuditLog, Clock, Outcome, PortError, RequestContext};
use zendiloos_notify::{Level, NewNotification, Notifier};

use crate::application::StorageError;
use crate::application::ports::{AdminError, BlockDeviceSource, RemovableHost};
use crate::domain::{self, RemovableDrive};

/// A drive as listed: what it is, and why it is not mounted if it is not.
#[derive(Debug, Clone, PartialEq)]
pub struct ListedDrive {
    pub drive: RemovableDrive,
    pub hold: Option<Hold>,
}

/// Why a drive that is plugged in is not mounted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// An administrator ejected it, without powering it off.
    Ejected,
    /// It could not be mounted, in the agent's words.
    Failed(String),
}

#[derive(Debug, Default)]
struct Watched {
    /// Mounted when last looked at, with their label: one that is gone
    /// next time was pulled out without being ejected.
    mounted: BTreeMap<String, Option<String>>,
    /// Plugged in, not to be mounted by themselves.
    held: BTreeMap<String, Hold>,
    /// Whether the places of drives from before zendiloosd started were
    /// cleaned up.
    cleaned: bool,
}

pub struct RemovableDrives {
    devices: Arc<dyn BlockDeviceSource>,
    host: Arc<dyn RemovableHost>,
    audit: Arc<dyn AuditLog>,
    clock: Arc<dyn Clock>,
    notifier: Notifier,
    /// One look or change at a time.
    watched: tokio::sync::Mutex<Watched>,
}

impl RemovableDrives {
    pub(crate) fn new(
        devices: Arc<dyn BlockDeviceSource>,
        host: Arc<dyn RemovableHost>,
        audit: Arc<dyn AuditLog>,
        clock: Arc<dyn Clock>,
        notifier: Notifier,
    ) -> Self {
        Self { devices, host, audit, clock, notifier, watched: tokio::sync::Mutex::default() }
    }

    /// The removable drives plugged in, after mounting the new ones.
    pub async fn list(&self, context: &RequestContext) -> Result<Vec<ListedDrive>, StorageError> {
        let mut watched = self.watched.lock().await;
        Ok(self.look(&mut watched, context).await?)
    }

    /// One look, for the regular round; failures are logged.
    pub async fn watch(&self) {
        let mut watched = self.watched.lock().await;
        if let Err(error) = self.look(&mut watched, &RequestContext::default()).await {
            tracing::warn!(%error, "removable drives: cannot look at the disks");
        }
    }

    async fn look(
        &self,
        watched: &mut Watched,
        context: &RequestContext,
    ) -> Result<Vec<ListedDrive>, PortError> {
        let mut drives = domain::removable_drives(&self.devices.block_devices(context).await?);
        let present: BTreeSet<String> = drives.iter().map(|drive| drive.id.clone()).collect();
        let pulled: Vec<(String, Option<String>)> = watched
            .mounted
            .iter()
            .filter(|(id, _)| !present.contains(*id))
            .map(|(i, l)| (i.clone(), l.clone()))
            .collect();
        watched.mounted.retain(|id, _| present.contains(id));
        watched.held.retain(|id, _| present.contains(id));
        if !pulled.is_empty() || !watched.cleaned {
            match self.host.clean(context).await {
                Ok(ids) => {
                    watched.cleaned = true;
                    if !ids.is_empty() {
                        tracing::info!(?ids, "removable drives: places let go of");
                    }
                }
                Err(error) => tracing::warn!(%error, "removable drives: cannot clean up"),
            }
        }
        for (id, label) in pulled {
            tracing::warn!(%id, "removable drive pulled out without being ejected");
            let name = label.unwrap_or(id);
            self.notifier
                .notify(NewNotification::new(Level::Warning, "removable_pulled").with("drive", name))
                .await;
        }
        for drive in &mut drives {
            if !drive.is_mounted() && !watched.held.contains_key(&drive.id) {
                match self.host.mount(drive, context).await {
                    Ok(read_only) => mounted(drive, read_only),
                    Err(error) => {
                        tracing::warn!(id = %drive.id, %error, "removable drive cannot be mounted");
                        watched.held.insert(drive.id.clone(), Hold::Failed(reason(error)));
                    }
                }
            }
            if drive.is_mounted() {
                watched.mounted.insert(drive.id.clone(), drive.label.clone());
            } else {
                watched.mounted.remove(&drive.id);
            }
        }
        Ok(drives
            .into_iter()
            .map(|drive| ListedDrive { hold: watched.held.get(&drive.id).cloned(), drive })
            .collect())
    }

    /// Mounts a drive that was ejected or could not be mounted; as it is
    /// if mounted already.
    pub async fn mount(
        &self,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<ListedDrive, StorageError> {
        let mut watched = self.watched.lock().await;
        let mut drive = self.find(id, context).await?;
        if !drive.is_mounted() {
            let outcome = self.host.mount(&drive, context).await;
            self.record("storage.mount_removable", outcome.is_ok(), id, actor, context).await;
            let read_only = outcome.map_err(removable_error)?;
            mounted(&mut drive, read_only);
        }
        watched.held.remove(id);
        watched.mounted.insert(drive.id.clone(), drive.label.clone());
        Ok(ListedDrive { drive, hold: None })
    }

    /// Unmounts a drive, refused while files on it are open; with
    /// `power_off` its disk goes off the bus, ready to be pulled out.
    /// Otherwise it stays plugged in, not mounted, until it is pulled out
    /// or mounted again.
    pub async fn eject(
        &self,
        id: &str,
        power_off: bool,
        actor: &Actor,
        context: &RequestContext,
    ) -> Result<(), StorageError> {
        let mut watched = self.watched.lock().await;
        let drive = self.find(id, context).await?;
        let outcome = self.host.eject(&drive, power_off, context).await;
        self.record("storage.eject_removable", outcome.is_ok(), id, actor, context).await;
        outcome.map_err(removable_error)?;
        watched.mounted.remove(id);
        watched.held.insert(id.to_owned(), Hold::Ejected);
        Ok(())
    }

    /// The drive `id` if it is mounted at its place now, as the disks say:
    /// what the file explorer may work on. Mounts nothing.
    pub async fn mounted(
        &self,
        id: &str,
        context: &RequestContext,
    ) -> Result<Option<RemovableDrive>, StorageError> {
        match self.find(id, context).await {
            Ok(drive) => Ok(drive.is_mounted().then_some(drive)),
            Err(StorageError::RemovableNotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn find(&self, id: &str, context: &RequestContext) -> Result<RemovableDrive, StorageError> {
        let devices = self.devices.block_devices(context).await?;
        domain::removable_drives(&devices)
            .into_iter()
            .find(|drive| drive.id == id)
            .ok_or(StorageError::RemovableNotFound)
    }

    async fn record(
        &self,
        action: &'static str,
        done: bool,
        id: &str,
        actor: &Actor,
        context: &RequestContext,
    ) {
        let outcome = if done { Outcome::Success } else { Outcome::Failure };
        let entry = AuditEntry::new(self.clock.now(), action, outcome, context).by(actor).target(id);
        if let Err(error) = self.audit.record(entry).await {
            tracing::error!(%error, action, "cannot write the audit log");
        }
    }
}

fn mounted(drive: &mut RemovableDrive, read_only: bool) {
    drive.mount_point = Some(domain::removable_place(&drive.id));
    drive.read_only |= read_only;
}

fn reason(error: AdminError) -> String {
    match error {
        AdminError::Refused(reason) => reason,
        AdminError::Conflict => "the drive is in use".to_owned(),
        AdminError::Port(error) => error.to_string(),
    }
}

/// The agent says "conflict" for a drive in use: files on it are open.
fn removable_error(error: AdminError) -> StorageError {
    match error {
        AdminError::Conflict => StorageError::DriveBusy,
        other => other.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use zendiloos_jobs::testing::FixedClock;
    use zendiloos_notify::testing::MemoryNotifications;

    use super::*;
    use crate::domain::{BlockDevice, DeviceKind, Filesystem};

    const ID: &str = "1A2B-3C4D";

    /// A USB stick that may be plugged in, and a host that mounts it.
    #[derive(Default)]
    struct Fake {
        plugged: Mutex<bool>,
        mounted: Mutex<bool>,
        refuse: Mutex<Option<AdminError>>,
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl BlockDeviceSource for Fake {
        async fn block_devices(&self, _: &RequestContext) -> Result<Vec<BlockDevice>, PortError> {
            if !*self.plugged.lock().unwrap() {
                return Ok(Vec::new());
            }
            let mount_points =
                if *self.mounted.lock().unwrap() { vec![domain::removable_place(ID)] } else { Vec::new() };
            let partition = BlockDevice {
                filesystem: Some(Filesystem {
                    kind: "vfat".into(),
                    label: Some("STICK".into()),
                    uuid: Some(ID.into()),
                    space: None,
                }),
                mount_points,
                ..device("sdf1", DeviceKind::Partition)
            };
            Ok(vec![BlockDevice {
                transport: Some("usb".into()),
                removable: true,
                children: vec![partition],
                ..device("sdf", DeviceKind::Disk)
            }])
        }
    }

    fn device(name: &str, kind: DeviceKind) -> BlockDevice {
        BlockDevice {
            name: name.into(),
            kind,
            size_bytes: 32 << 30,
            model: None,
            serial: None,
            wwn: None,
            id_link: None,
            transport: None,
            rotational: false,
            removable: false,
            read_only: false,
            filesystem: None,
            mount_points: Vec::new(),
            children: Vec::new(),
        }
    }

    #[async_trait]
    impl RemovableHost for Fake {
        async fn mount(&self, drive: &RemovableDrive, _: &RequestContext) -> Result<bool, AdminError> {
            self.calls.lock().unwrap().push(format!("mount {}", drive.id));
            if let Some(error) = self.refuse.lock().unwrap().take() {
                return Err(error);
            }
            *self.mounted.lock().unwrap() = true;
            Ok(false)
        }

        async fn eject(
            &self,
            drive: &RemovableDrive,
            power_off: bool,
            _: &RequestContext,
        ) -> Result<(), AdminError> {
            self.calls.lock().unwrap().push(format!("eject {} {power_off}", drive.id));
            if let Some(error) = self.refuse.lock().unwrap().take() {
                return Err(error);
            }
            *self.mounted.lock().unwrap() = false;
            if power_off {
                *self.plugged.lock().unwrap() = false;
            }
            Ok(())
        }

        async fn clean(&self, _: &RequestContext) -> Result<Vec<String>, AdminError> {
            self.calls.lock().unwrap().push("clean".into());
            Ok(Vec::new())
        }
    }

    struct NoAudit;

    #[async_trait]
    impl AuditLog for NoAudit {
        async fn record(&self, _: AuditEntry) -> Result<(), PortError> {
            Ok(())
        }
    }

    fn drives(fake: &Arc<Fake>) -> (RemovableDrives, Arc<MemoryNotifications>) {
        let notifications = Arc::new(MemoryNotifications::default());
        let notifier = Notifier::new(notifications.clone(), Arc::new(FixedClock));
        let drives = RemovableDrives::new(
            fake.clone(),
            fake.clone(),
            Arc::new(NoAudit),
            Arc::new(FixedClock),
            notifier,
        );
        (drives, notifications)
    }

    fn context() -> RequestContext {
        RequestContext::default()
    }

    fn admin() -> Actor {
        Actor { account_id: 1, name: "alice".into() }
    }

    #[tokio::test]
    async fn a_stick_plugged_in_is_mounted_when_seen_once() {
        let fake = Arc::new(Fake::default());
        let (drives, _) = drives(&fake);
        assert_eq!(drives.list(&context()).await.unwrap(), []);
        *fake.plugged.lock().unwrap() = true;

        let listed = drives.list(&context()).await.unwrap();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].drive.mount_point.as_deref(), Some("/run/zendiloos-removable/1A2B-3C4D"));
        assert_eq!(listed[0].hold, None);
        drives.watch().await;
        assert_eq!(fake.calls(), ["clean", "mount 1A2B-3C4D"], "cleaned once at the start, mounted once");
    }

    #[tokio::test]
    async fn a_drive_is_worked_on_only_while_it_is_mounted() {
        let fake = Arc::new(Fake::default());
        let (drives, _) = drives(&fake);
        assert_eq!(drives.mounted(ID, &context()).await.unwrap(), None, "not plugged in");
        *fake.plugged.lock().unwrap() = true;
        assert_eq!(drives.mounted(ID, &context()).await.unwrap(), None, "plugged in, not mounted yet");
        assert!(fake.calls().is_empty(), "asking mounts nothing");
        drives.list(&context()).await.unwrap();
        let drive = drives.mounted(ID, &context()).await.unwrap().unwrap();
        assert_eq!((drive.id.as_str(), drive.label.as_deref()), (ID, Some("STICK")));
        assert_eq!(drives.mounted("other", &context()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn an_ejected_stick_stays_unmounted_until_mounted_again() {
        let fake = Arc::new(Fake::default());
        *fake.plugged.lock().unwrap() = true;
        let (drives, notifications) = drives(&fake);
        drives.list(&context()).await.unwrap();

        drives.eject(ID, false, &admin(), &context()).await.unwrap();
        let listed = drives.list(&context()).await.unwrap();

        assert_eq!((listed[0].drive.is_mounted(), listed[0].hold.clone()), (false, Some(Hold::Ejected)));
        let again = drives.mount(ID, &admin(), &context()).await.unwrap();
        assert!(again.drive.is_mounted());
        assert!(drives.list(&context()).await.unwrap()[0].hold.is_none());
        assert_eq!(fake.calls(), ["clean", "mount 1A2B-3C4D", "eject 1A2B-3C4D false", "mount 1A2B-3C4D"]);
        // Powered off, it is gone without a word.
        drives.eject(ID, true, &admin(), &context()).await.unwrap();
        assert_eq!(drives.list(&context()).await.unwrap(), []);
        assert_eq!(notifications.all(), []);
    }

    #[tokio::test]
    async fn a_stick_pulled_out_while_mounted_is_reported_and_cleaned_up() {
        let fake = Arc::new(Fake::default());
        *fake.plugged.lock().unwrap() = true;
        let (drives, notifications) = drives(&fake);
        drives.list(&context()).await.unwrap();

        *fake.plugged.lock().unwrap() = false;
        drives.watch().await;

        let told = notifications.all();
        assert_eq!(told.len(), 1);
        assert_eq!((told[0].code.as_str(), told[0].params["drive"].as_str()), ("removable_pulled", "STICK"));
        assert_eq!(fake.calls(), ["clean", "mount 1A2B-3C4D", "clean"]);
    }

    #[tokio::test]
    async fn a_drive_that_cannot_be_mounted_says_why_and_is_not_tried_again() {
        let fake = Arc::new(Fake::default());
        *fake.plugged.lock().unwrap() = true;
        *fake.refuse.lock().unwrap() = Some(AdminError::Refused("sdf2 is in use at /".into()));
        let (drives, _) = drives(&fake);

        let listed = drives.list(&context()).await.unwrap();
        drives.watch().await;

        assert_eq!(listed[0].hold, Some(Hold::Failed("sdf2 is in use at /".into())));
        assert_eq!(fake.calls().iter().filter(|call| call.starts_with("mount")).count(), 1);
    }

    #[tokio::test]
    async fn a_drive_in_use_is_not_ejected_and_unknown_ones_are_not_found() {
        let fake = Arc::new(Fake::default());
        *fake.plugged.lock().unwrap() = true;
        let (drives, _) = drives(&fake);
        drives.list(&context()).await.unwrap();
        *fake.refuse.lock().unwrap() = Some(AdminError::Conflict);

        let busy = drives.eject(ID, true, &admin(), &context()).await;

        assert!(matches!(busy, Err(StorageError::DriveBusy)), "{busy:?}");
        assert!(drives.list(&context()).await.unwrap()[0].drive.is_mounted());
        let unknown = drives.eject("9F00-0001", true, &admin(), &context()).await;
        assert!(matches!(unknown, Err(StorageError::RemovableNotFound)), "{unknown:?}");
        let unknown = drives.mount("9F00-0001", &admin(), &context()).await;
        assert!(matches!(unknown, Err(StorageError::RemovableNotFound)), "{unknown:?}");
        *fake.refuse.lock().unwrap() = Some(AdminError::Refused("no".into()));
        drives.eject(ID, false, &admin(), &context()).await.unwrap_err();
    }
}
