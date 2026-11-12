//! The virtual machines' settings, with their disks (each on its location),
//! network cards and CD drives; the locations of their storage; and the
//! image library.

use std::collections::BTreeMap;

use async_trait::async_trait;
use sqlx::{Sqlite, SqliteConnection, Transaction};
use zendiloos_kernel::{PortError, Timestamp};
use zendiloos_vms::application::{
    VmBackupRepository, VmImageRepository, VmLocationRepository, VmRepository, VmScheduleRepository,
};
use zendiloos_vms::domain::{
    BackupFile, BackupPart, BackupPlace, BootDevice, Consistency, Disk, DiskBus, Drive, Firmware, Frequency,
    ImageOrigin, Location, Network, NewVmBackup, Nic, NicModel, OsPreset, SnapshotSchedule, UsbDevice, Vm,
    VmBackup, VmImage, VmSettings,
};

use super::Database;

/// [`VmRepository`] on the `vms` table and its devices' tables.
#[derive(Debug, Clone)]
pub struct SqliteVms {
    db: Database,
}

impl SqliteVms {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn db_error(error: sqlx::Error) -> PortError {
    PortError::new("virtual machines", error)
}

fn stored(what: String) -> PortError {
    PortError::new("stored virtual machine", what)
}

/// Whether `error` is the name of a VM or an image being taken.
fn name_taken(error: &sqlx::Error, table: &str) -> bool {
    error.as_database_error().is_some_and(|error| {
        error.is_unique_violation() && error.message().contains(&format!("{table}.name"))
    })
}

struct Row {
    id: String,
    name: String,
    os: String,
    cpus: i64,
    memory_mib: i64,
    firmware: String,
    autostart: bool,
    boot: String,
    created_at: i64,
    ready: bool,
    tpm: bool,
    secure_boot: bool,
}

struct DiskRow {
    vm: String,
    slot: i64,
    size_gib: i64,
    bus: String,
    pool: String,
    volume: i64,
}

struct NicRow {
    vm: String,
    slot: i64,
    model: String,
    network: String,
    device: Option<String>,
    mac: String,
}

struct DriveRow {
    vm: String,
    slot: i64,
    image: Option<String>,
}

struct HostDeviceRow {
    vm: String,
    kind: String,
    device: String,
}

/// Each VM's devices, by its id.
#[derive(Default)]
struct Devices {
    disks: BTreeMap<String, Vec<Disk>>,
    nics: BTreeMap<String, Vec<Nic>>,
    drives: BTreeMap<String, Vec<Drive>>,
    usb: BTreeMap<String, Vec<UsbDevice>>,
    pci: BTreeMap<String, Vec<String>>,
}

fn slot(value: i64, of: &str) -> Result<u8, PortError> {
    u8::try_from(value).map_err(|_| stored(format!("slot {value} of {of}")))
}

impl Devices {
    fn of(
        disks: Vec<DiskRow>,
        nics: Vec<NicRow>,
        drives: Vec<DriveRow>,
        host: Vec<HostDeviceRow>,
    ) -> Result<Self, PortError> {
        let mut devices = Self::default();
        for row in host {
            match row.kind.as_str() {
                "usb" => {
                    let device = UsbDevice::parse(&row.device)
                        .ok_or_else(|| stored(format!("USB device {}", row.device)))?;
                    devices.usb.entry(row.vm).or_default().push(device);
                }
                "pci" => devices.pci.entry(row.vm).or_default().push(row.device),
                other => return Err(stored(format!("host device of kind {other}"))),
            }
        }
        for row in disks {
            let disk = Disk {
                slot: slot(row.slot, &row.vm)?,
                size_gib: u32::try_from(row.size_gib)
                    .map_err(|_| stored(format!("disk size of {}", row.vm)))?,
                bus: DiskBus::parse(&row.bus).ok_or_else(|| stored(format!("bus {}", row.bus)))?,
                location: Location {
                    volume: u16::try_from(row.volume)
                        .map_err(|_| stored(format!("volume of a disk of {}", row.vm)))?,
                    pool: row.pool,
                },
            };
            devices.disks.entry(row.vm).or_default().push(disk);
        }
        for row in nics {
            let nic = Nic {
                slot: slot(row.slot, &row.vm)?,
                model: NicModel::parse(&row.model).ok_or_else(|| stored(format!("model {}", row.model)))?,
                network: Network::of(&row.network, row.device)
                    .ok_or_else(|| stored(format!("network {} of {}", row.network, row.vm)))?,
                mac: row.mac,
            };
            devices.nics.entry(row.vm).or_default().push(nic);
        }
        for row in drives {
            let drive = Drive { slot: slot(row.slot, &row.vm)?, image: row.image };
            devices.drives.entry(row.vm).or_default().push(drive);
        }
        Ok(devices)
    }

    fn vm(&mut self, row: Row) -> Result<Vm, PortError> {
        let number = |value: i64, what: &str| stored(format!("{what} {value} of {}", row.id));
        let boot = row
            .boot
            .split_whitespace()
            .map(|device| BootDevice::parse(device).ok_or_else(|| stored(format!("boot device {device}"))))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Vm {
            settings: VmSettings {
                name: row.name,
                os: OsPreset::parse(&row.os).ok_or_else(|| stored(format!("os {}", row.os)))?,
                cpus: u16::try_from(row.cpus).map_err(|_| number(row.cpus, "cpus"))?,
                memory_mib: u32::try_from(row.memory_mib).map_err(|_| number(row.memory_mib, "memory"))?,
                firmware: Firmware::parse(&row.firmware)
                    .ok_or_else(|| stored(format!("firmware {}", row.firmware)))?,
                disks: self.disks.remove(&row.id).unwrap_or_default(),
                nics: self.nics.remove(&row.id).unwrap_or_default(),
                drives: self.drives.remove(&row.id).unwrap_or_default(),
                boot,
                autostart: row.autostart,
                usb: self.usb.remove(&row.id).unwrap_or_default(),
                pci: self.pci.remove(&row.id).unwrap_or_default(),
                tpm: row.tpm,
                secure_boot: row.secure_boot,
            },
            created_at: Timestamp::from_unix_seconds(row.created_at),
            ready: row.ready,
            id: row.id,
        })
    }
}

fn boot_text(boot: &[BootDevice]) -> String {
    boot.iter().map(|device| device.to_text()).collect::<Vec<_>>().join(" ")
}

impl SqliteVms {
    /// The devices of one VM, or of all, read on `connection`: within the
    /// transaction that read the VMs, so both come from the same moment.
    async fn devices(connection: &mut SqliteConnection, id: Option<&str>) -> Result<Devices, PortError> {
        let disks = sqlx::query_as!(
            DiskRow,
            r#"SELECT vm AS "vm!", slot, size_gib, bus, pool, volume FROM vm_disks
               WHERE ?1 IS NULL OR vm = ?1 ORDER BY vm, slot"#,
            id
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(db_error)?;
        let nics = sqlx::query_as!(
            NicRow,
            r#"SELECT vm AS "vm!", slot, model, network, device, mac FROM vm_nics
               WHERE ?1 IS NULL OR vm = ?1 ORDER BY vm, slot"#,
            id
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(db_error)?;
        let drives = sqlx::query_as!(
            DriveRow,
            r#"SELECT vm AS "vm!", slot, image FROM vm_drives
               WHERE ?1 IS NULL OR vm = ?1 ORDER BY vm, slot"#,
            id
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(db_error)?;
        let host = sqlx::query_as!(
            HostDeviceRow,
            r#"SELECT vm AS "vm!", kind AS "kind!", device AS "device!" FROM vm_host_devices
               WHERE ?1 IS NULL OR vm = ?1 ORDER BY vm, kind, device"#,
            id
        )
        .fetch_all(&mut *connection)
        .await
        .map_err(db_error)?;
        Devices::of(disks, nics, drives, host)
    }

    /// The VM's devices, as its settings have them, in place of those it had.
    async fn write_devices(transaction: &mut Transaction<'_, Sqlite>, vm: &Vm) -> Result<(), sqlx::Error> {
        let id = vm.id.as_str();
        sqlx::query!("DELETE FROM vm_disks WHERE vm = ?", id).execute(&mut **transaction).await?;
        sqlx::query!("DELETE FROM vm_nics WHERE vm = ?", id).execute(&mut **transaction).await?;
        sqlx::query!("DELETE FROM vm_drives WHERE vm = ?", id).execute(&mut **transaction).await?;
        sqlx::query!("DELETE FROM vm_host_devices WHERE vm = ?", id).execute(&mut **transaction).await?;
        let usb = vm.settings.usb.iter().map(|device| ("usb", device.id()));
        let pci = vm.settings.pci.iter().map(|address| ("pci", address.clone()));
        for (kind, device) in usb.chain(pci) {
            sqlx::query!("INSERT INTO vm_host_devices (vm, kind, device) VALUES (?, ?, ?)", id, kind, device)
                .execute(&mut **transaction)
                .await?;
        }
        for disk in &vm.settings.disks {
            let (slot, size, bus) = (i64::from(disk.slot), i64::from(disk.size_gib), disk.bus.as_str());
            let volume = i64::from(disk.location.volume);
            sqlx::query!(
                "INSERT INTO vm_disks (vm, slot, size_gib, bus, pool, volume) VALUES (?, ?, ?, ?, ?, ?)",
                id,
                slot,
                size,
                bus,
                disk.location.pool,
                volume
            )
            .execute(&mut **transaction)
            .await?;
        }
        for nic in &vm.settings.nics {
            let (slot, model) = (i64::from(nic.slot), nic.model.as_str());
            let (network, device) = (nic.network.kind(), nic.network.device());
            sqlx::query!(
                "INSERT INTO vm_nics (vm, slot, model, network, device, mac) VALUES (?, ?, ?, ?, ?, ?)",
                id,
                slot,
                model,
                network,
                device,
                nic.mac
            )
            .execute(&mut **transaction)
            .await?;
        }
        for drive in &vm.settings.drives {
            let slot = i64::from(drive.slot);
            sqlx::query!("INSERT INTO vm_drives (vm, slot, image) VALUES (?, ?, ?)", id, slot, drive.image)
                .execute(&mut **transaction)
                .await?;
        }
        Ok(())
    }
}

#[async_trait]
impl VmRepository for SqliteVms {
    /// Read in one transaction, which SQLite gives one view of the
    /// database: a VM is never read with the devices of a change written
    /// meanwhile.
    async fn list(&self) -> Result<Vec<Vm>, PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", name, os, cpus, memory_mib, firmware, autostart AS "autostart: bool", boot,
                      created_at, ready AS "ready: bool", tpm AS "tpm: bool", secure_boot AS "secure_boot: bool"
               FROM vms ORDER BY created_at, id"#
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(db_error)?;
        let mut devices = Self::devices(&mut transaction, None).await?;
        rows.into_iter().map(|row| devices.vm(row)).collect()
    }

    /// In one transaction, as [`Self::list`].
    async fn find(&self, id: &str) -> Result<Option<Vm>, PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let row = sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!", name, os, cpus, memory_mib, firmware, autostart AS "autostart: bool", boot,
                      created_at, ready AS "ready: bool", tpm AS "tpm: bool", secure_boot AS "secure_boot: bool"
               FROM vms WHERE id = ?"#,
            id
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(db_error)?;
        let Some(row) = row else { return Ok(None) };
        let mut devices = Self::devices(&mut transaction, Some(id)).await?;
        devices.vm(row).map(Some)
    }

    async fn create(&self, vm: &Vm) -> Result<bool, PortError> {
        let settings = &vm.settings;
        let (os, firmware) = (settings.os.as_str(), settings.firmware.as_str());
        let (cpus, memory) = (i64::from(settings.cpus), i64::from(settings.memory_mib));
        let boot = boot_text(&settings.boot);
        let created_at = vm.created_at.unix_seconds();
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let done = sqlx::query!(
            "INSERT INTO vms (id, name, os, cpus, memory_mib, firmware, autostart, boot, created_at, ready, tpm,
                              secure_boot)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            vm.id,
            settings.name,
            os,
            cpus,
            memory,
            firmware,
            settings.autostart,
            boot,
            created_at,
            vm.ready,
            settings.tpm,
            settings.secure_boot
        )
        .execute(&mut *transaction)
        .await
        .map_err(db_error)?;
        if done.rows_affected() != 1 {
            return Ok(false);
        }
        Self::write_devices(&mut transaction, vm).await.map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        Ok(true)
    }

    async fn set_ready(&self, id: &str) -> Result<(), PortError> {
        sqlx::query!("UPDATE vms SET ready = 1 WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn set_autostart(&self, id: &str, autostart: bool) -> Result<bool, PortError> {
        let done = sqlx::query!("UPDATE vms SET autostart = ? WHERE id = ?", autostart, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    async fn update(&self, vm: &Vm) -> Result<bool, PortError> {
        let settings = &vm.settings;
        let firmware = settings.firmware.as_str();
        let (cpus, memory) = (i64::from(settings.cpus), i64::from(settings.memory_mib));
        let boot = boot_text(&settings.boot);
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        let done = sqlx::query!(
            "UPDATE vms SET name = ?, cpus = ?, memory_mib = ?, firmware = ?, autostart = ?, boot = ?, tpm = ?,
                            secure_boot = ?
             WHERE id = ?",
            settings.name,
            cpus,
            memory,
            firmware,
            settings.autostart,
            boot,
            settings.tpm,
            settings.secure_boot,
            vm.id
        )
        .execute(&mut *transaction)
        .await;
        match done {
            Ok(done) if done.rows_affected() == 1 => {}
            Ok(_) => return Ok(false),
            Err(error) if name_taken(&error, "vms") => return Ok(false),
            Err(error) => return Err(db_error(error)),
        }
        Self::write_devices(&mut transaction, vm).await.map_err(db_error)?;
        transaction.commit().await.map_err(db_error)?;
        Ok(true)
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        let done = sqlx::query!("DELETE FROM vms WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }
}

/// [`VmScheduleRepository`] on the `vm_snapshot_schedules` table.
#[derive(Debug, Clone)]
pub struct SqliteVmSchedules {
    db: Database,
}

impl SqliteVmSchedules {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

fn schedule(frequency: &str, keep: i64) -> Result<SnapshotSchedule, PortError> {
    Ok(SnapshotSchedule {
        frequency: Frequency::parse(frequency).ok_or_else(|| stored(format!("frequency {frequency}")))?,
        keep: u32::try_from(keep).map_err(|_| stored(format!("keep {keep}")))?,
    })
}

#[async_trait]
impl VmScheduleRepository for SqliteVmSchedules {
    async fn list(&self, vm: &str) -> Result<Vec<SnapshotSchedule>, PortError> {
        let rows = sqlx::query!(r#"SELECT frequency, keep FROM vm_snapshot_schedules WHERE vm = ?"#, vm)
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        let mut list =
            rows.into_iter().map(|row| schedule(&row.frequency, row.keep)).collect::<Result<Vec<_>, _>>()?;
        list.sort_by_key(|schedule| schedule.frequency);
        Ok(list)
    }

    async fn all(&self) -> Result<BTreeMap<String, Vec<SnapshotSchedule>>, PortError> {
        let rows =
            sqlx::query!(r#"SELECT vm AS "vm!", frequency, keep FROM vm_snapshot_schedules ORDER BY vm"#)
                .fetch_all(self.db.pool())
                .await
                .map_err(db_error)?;
        let mut all: BTreeMap<String, Vec<SnapshotSchedule>> = BTreeMap::new();
        for row in rows {
            all.entry(row.vm).or_default().push(schedule(&row.frequency, row.keep)?);
        }
        for list in all.values_mut() {
            list.sort_by_key(|schedule| schedule.frequency);
        }
        Ok(all)
    }

    async fn set(&self, vm: &str, schedules: &[SnapshotSchedule]) -> Result<(), PortError> {
        let mut transaction = self.db.pool().begin().await.map_err(db_error)?;
        sqlx::query!("DELETE FROM vm_snapshot_schedules WHERE vm = ?", vm)
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        for schedule in schedules {
            let (frequency, keep) = (schedule.frequency.as_str(), i64::from(schedule.keep));
            sqlx::query!(
                "INSERT INTO vm_snapshot_schedules (vm, frequency, keep) VALUES (?, ?, ?)",
                vm,
                frequency,
                keep
            )
            .execute(&mut *transaction)
            .await
            .map_err(db_error)?;
        }
        transaction.commit().await.map_err(db_error)
    }
}

/// [`VmBackupRepository`] on the `vm_backups` table; the files and the
/// VM's settings as JSON.
#[derive(Debug, Clone)]
pub struct SqliteVmBackups {
    db: Database,
}

impl SqliteVmBackups {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FileJson {
    part: String,
    #[serde(default)]
    slot: Option<u8>,
    #[serde(default)]
    volsize: Option<u64>,
    bytes: u64,
    sha256: String,
}

fn files_json(files: &[BackupFile]) -> String {
    let files: Vec<FileJson> = files
        .iter()
        .map(|file| {
            let (part, slot, volsize) = match file.part {
                BackupPart::Disk { slot, volsize } => ("disk", Some(slot), Some(volsize)),
                BackupPart::Nvram => ("nvram", None, None),
                BackupPart::Tpm => ("tpm", None, None),
            };
            FileJson { part: part.to_owned(), slot, volsize, bytes: file.bytes, sha256: file.sha256.clone() }
        })
        .collect();
    serde_json::to_string(&files).unwrap_or_default()
}

fn files_of(text: &str) -> Result<Vec<BackupFile>, PortError> {
    let files: Vec<FileJson> =
        serde_json::from_str(text).map_err(|e| stored(format!("a backup's files: {e}")))?;
    files
        .into_iter()
        .map(|file| {
            let part = match (file.part.as_str(), file.slot, file.volsize) {
                ("disk", Some(slot), Some(volsize)) => BackupPart::Disk { slot, volsize },
                ("nvram", _, _) => BackupPart::Nvram,
                ("tpm", _, _) => BackupPart::Tpm,
                (other, _, _) => return Err(stored(format!("a backup's file {other}"))),
            };
            Ok(BackupFile { part, bytes: file.bytes, sha256: file.sha256 })
        })
        .collect()
}

struct BackupRow {
    id: i64,
    vm: String,
    vm_name: String,
    place: String,
    share_id: Option<i64>,
    share: Option<String>,
    volume: Option<i64>,
    drive: Option<String>,
    folder: String,
    taken_at: i64,
    consistency: String,
    files: String,
    settings: String,
}

fn backup(row: BackupRow) -> Result<VmBackup, PortError> {
    let place = match (row.place.as_str(), row.share_id, row.share, row.volume, row.drive) {
        ("share", Some(share_id), Some(share), Some(volume), _) => BackupPlace::Share {
            share_id,
            share,
            volume: u16::try_from(volume).map_err(|_| stored(format!("volume of backup {}", row.id)))?,
        },
        ("drive", _, _, _, Some(drive)) => BackupPlace::Drive { drive },
        (other, ..) => return Err(stored(format!("place {other} of backup {}", row.id))),
    };
    Ok(VmBackup {
        id: row.id,
        vm: row.vm,
        vm_name: row.vm_name,
        place,
        folder: row.folder,
        taken_at: Timestamp::from_unix_seconds(row.taken_at),
        consistency: Consistency::parse(&row.consistency)
            .ok_or_else(|| stored(format!("consistency {}", row.consistency)))?,
        files: files_of(&row.files)?,
        settings: crate::vms::settings::from_json(&row.settings).map_err(stored)?,
    })
}

#[async_trait]
impl VmBackupRepository for SqliteVmBackups {
    async fn list(&self) -> Result<Vec<VmBackup>, PortError> {
        let rows = sqlx::query_as!(
            BackupRow,
            r#"SELECT id AS "id!", vm, vm_name, place, share_id, share, volume, drive, folder, taken_at, consistency,
                      files, settings
               FROM vm_backups ORDER BY taken_at DESC, id DESC"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter().map(backup).collect()
    }

    async fn find(&self, id: i64) -> Result<Option<VmBackup>, PortError> {
        let row = sqlx::query_as!(
            BackupRow,
            r#"SELECT id AS "id!", vm, vm_name, place, share_id, share, volume, drive, folder, taken_at, consistency,
                      files, settings
               FROM vm_backups WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        row.map(backup).transpose()
    }

    async fn create(&self, new: &NewVmBackup) -> Result<i64, PortError> {
        let (place, share_id, share, volume, drive) = match &new.place {
            BackupPlace::Share { share_id, share, volume } => {
                ("share", Some(*share_id), Some(share.clone()), Some(i64::from(*volume)), None)
            }
            BackupPlace::Drive { drive } => ("drive", None, None, None, Some(drive.clone())),
        };
        let taken_at = new.taken_at.unix_seconds();
        let consistency = new.consistency.as_str();
        let files = files_json(&new.files);
        let settings = crate::vms::settings::to_json(&new.settings);
        let done = sqlx::query!(
            "INSERT INTO vm_backups (vm, vm_name, place, share_id, share, volume, drive, folder, taken_at, consistency,
                                     files, settings)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            new.vm,
            new.vm_name,
            place,
            share_id,
            share,
            volume,
            drive,
            new.folder,
            taken_at,
            consistency,
            files,
            settings
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(done.last_insert_rowid())
    }

    async fn delete(&self, id: i64) -> Result<bool, PortError> {
        let done = sqlx::query!("DELETE FROM vm_backups WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }
}

/// [`VmLocationRepository`] on the `vm_locations` table.
#[derive(Debug, Clone)]
pub struct SqliteVmLocations {
    db: Database,
}

impl SqliteVmLocations {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

#[async_trait]
impl VmLocationRepository for SqliteVmLocations {
    async fn list(&self) -> Result<Vec<Location>, PortError> {
        let rows = sqlx::query!(r#"SELECT pool AS "pool!", volume FROM vm_locations ORDER BY position"#)
            .fetch_all(self.db.pool())
            .await
            .map_err(db_error)?;
        rows.into_iter()
            .map(|row| {
                let volume = u16::try_from(row.volume)
                    .map_err(|_| stored(format!("volume of location {}", row.pool)))?;
                Ok(Location { pool: row.pool, volume })
            })
            .collect()
    }

    async fn add(&self, location: &Location) -> Result<(), PortError> {
        let volume = i64::from(location.volume);
        sqlx::query!(
            "INSERT INTO vm_locations (pool, volume, position)
             VALUES (?, ?, (SELECT coalesce(max(position), 0) + 1 FROM vm_locations)) ON CONFLICT DO NOTHING",
            location.pool,
            volume
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(())
    }

    async fn remove(&self, pool: &str) -> Result<bool, PortError> {
        let done = sqlx::query!("DELETE FROM vm_locations WHERE pool = ?", pool)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }
}

/// [`VmImageRepository`] on the `vm_images` table.
#[derive(Debug, Clone)]
pub struct SqliteVmImages {
    db: Database,
}

impl SqliteVmImages {
    pub fn new(db: Database) -> Self {
        Self { db }
    }
}

struct ImageRow {
    id: String,
    name: String,
    bytes: Option<i64>,
    origin: String,
    share: Option<String>,
    path: String,
    volume: i64,
    added_at: i64,
    ready: bool,
}

fn image(row: ImageRow) -> Result<VmImage, PortError> {
    let origin = match (row.origin.as_str(), row.share) {
        ("share", Some(share)) => ImageOrigin::Share { share, path: row.path },
        ("upload", _) => ImageOrigin::Upload { file: row.path },
        (other, _) => return Err(stored(format!("origin {other} of image {}", row.id))),
    };
    Ok(VmImage {
        name: row.name,
        bytes: row.bytes.map(|bytes| u64::try_from(bytes).unwrap_or(0)),
        origin,
        volume: u16::try_from(row.volume).map_err(|_| stored(format!("volume of image {}", row.id)))?,
        added_at: Timestamp::from_unix_seconds(row.added_at),
        ready: row.ready,
        id: row.id,
    })
}

fn bytes(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

#[async_trait]
impl VmImageRepository for SqliteVmImages {
    async fn list(&self) -> Result<Vec<VmImage>, PortError> {
        let rows = sqlx::query_as!(
            ImageRow,
            r#"SELECT id AS "id!", name, bytes, origin, share, path, volume, added_at, ready AS "ready: bool"
               FROM vm_images ORDER BY added_at, id"#
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(db_error)?;
        rows.into_iter().map(image).collect()
    }

    async fn find(&self, id: &str) -> Result<Option<VmImage>, PortError> {
        let row = sqlx::query_as!(
            ImageRow,
            r#"SELECT id AS "id!", name, bytes, origin, share, path, volume, added_at, ready AS "ready: bool"
               FROM vm_images WHERE id = ?"#,
            id
        )
        .fetch_optional(self.db.pool())
        .await
        .map_err(db_error)?;
        row.map(image).transpose()
    }

    async fn create(&self, image: &VmImage) -> Result<bool, PortError> {
        let (origin, share, path) = match &image.origin {
            ImageOrigin::Share { share, path } => ("share", Some(share.as_str()), path.as_str()),
            ImageOrigin::Upload { file } => ("upload", None, file.as_str()),
        };
        let size = image.bytes.map(bytes);
        let (volume, added_at) = (i64::from(image.volume), image.added_at.unix_seconds());
        let done = sqlx::query!(
            "INSERT INTO vm_images (id, name, bytes, origin, share, path, volume, added_at, ready)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            image.id,
            image.name,
            size,
            origin,
            share,
            path,
            volume,
            added_at,
            image.ready
        )
        .execute(self.db.pool())
        .await
        .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }

    async fn set_ready(&self, id: &str, size: u64) -> Result<(), PortError> {
        let size = bytes(size);
        sqlx::query!("UPDATE vm_images SET ready = 1, bytes = ? WHERE id = ?", size, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn set_bytes(&self, id: &str, size: u64) -> Result<(), PortError> {
        let size = bytes(size);
        sqlx::query!("UPDATE vm_images SET bytes = ? WHERE id = ?", size, id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn rename(&self, id: &str, name: &str) -> Result<bool, PortError> {
        match sqlx::query!("UPDATE vm_images SET name = ? WHERE id = ?", name, id)
            .execute(self.db.pool())
            .await
        {
            Ok(done) => Ok(done.rows_affected() == 1),
            Err(error) if name_taken(&error, "vm_images") => Ok(false),
            Err(error) => Err(db_error(error)),
        }
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        let done = sqlx::query!("DELETE FROM vm_images WHERE id = ?", id)
            .execute(self.db.pool())
            .await
            .map_err(db_error)?;
        Ok(done.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::tests::temporary_database;

    fn image(id: &str, name: &str) -> VmImage {
        VmImage {
            id: id.into(),
            name: name.into(),
            bytes: None,
            origin: ImageOrigin::Share { share: "media".into(), path: "isos/Win11.iso".into() },
            volume: 1,
            added_at: Timestamp::from_unix_seconds(1_700_000_000),
            ready: false,
        }
    }

    fn vm(id: &str, name: &str) -> Vm {
        let mut settings = VmSettings {
            nics: vec![Nic {
                slot: 0,
                model: NicModel::E1000e,
                network: Network::Bridge("br0".into()),
                mac: format!("52:54:00:00:00:{}", &id[10..]),
            }],
            ..VmSettings::preset(name, OsPreset::Windows)
        };
        settings.disks[0].location = Location { pool: "tank".into(), volume: 1 };
        settings.disks.push(Disk {
            slot: 3,
            size_gib: 100,
            bus: DiskBus::Virtio,
            location: Location { pool: "cold".into(), volume: 2 },
        });
        Vm { id: id.into(), settings, created_at: Timestamp::from_unix_seconds(1_700_000_000), ready: false }
    }

    /// Each change of `busy` writes its record whole, its memory and its
    /// cards' addresses naming the change (`k`): read while it changes, a
    /// VM is one change's, never a record of one with the cards of another;
    /// and a VM that is not changed reads as it was, cards and all.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_vm_is_read_whole_while_it_and_others_change() {
        let (_dir, db) = temporary_database().await;
        let vms = std::sync::Arc::new(SqliteVms::new(db.clone()));
        let steady = vm("00000000000a", "steady");
        assert!(vms.create(&steady).await.unwrap());
        fn written(k: u32) -> Vm {
            let mut busy = vm("00000000000b", "busy");
            busy.settings.memory_mib = 256 + k;
            // One card or three, every one naming the change.
            busy.settings.nics = (0..if k.is_multiple_of(2) { 1 } else { 3 })
                .map(|slot| Nic {
                    slot,
                    model: NicModel::Virtio,
                    network: Network::User,
                    mac: format!("52:54:00:{:02x}:{:02x}:{slot:02x}", (k >> 8) & 0xff, k & 0xff),
                })
                .collect();
            busy
        }
        fn whole(busy: &Vm) -> bool {
            busy.settings == written(busy.settings.memory_mib - 256).settings
        }
        assert!(vms.create(&written(0)).await.unwrap());
        let writer = {
            let vms = vms.clone();
            tokio::spawn(async move {
                for k in 1..400 {
                    assert!(vms.update(&written(k)).await.unwrap());
                }
            })
        };
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (vms, steady) = (vms.clone(), steady.clone());
                tokio::spawn(async move {
                    for _ in 0..300 {
                        let busy = vms.find("00000000000b").await.unwrap().unwrap();
                        assert!(whole(&busy), "a record of one change with the cards of another: {busy:?}");
                        let all = vms.list().await.unwrap();
                        assert_eq!(all[0].settings, steady.settings, "the VM nobody changes");
                        assert!(whole(&all[1]), "listed half changed: {:?}", all[1]);
                    }
                })
            })
            .collect();
        writer.await.unwrap();
        for reader in readers {
            reader.await.unwrap();
        }
        db.close().await;
    }

    #[tokio::test]
    async fn vms_are_kept_with_their_settings_and_devices() {
        let (_dir, db) = temporary_database().await;
        let vms = SqliteVms::new(db.clone());
        let images = SqliteVmImages::new(db);
        assert!(images.create(&image("00000000000a", "Win11.iso")).await.unwrap());
        let mut first = vm("0a1b2c3d4e5f", "Büro-PC");
        first.settings.drives =
            vec![Drive { slot: 0, image: Some("00000000000a".into()) }, Drive { slot: 2, image: None }];
        assert!(vms.create(&first).await.unwrap());
        assert!(!vms.create(&vm("0a1b2c3d4e5f", "other")).await.unwrap(), "the id is taken");
        assert!(!vms.create(&vm("00000000000c", "Büro-PC")).await.unwrap(), "the name is taken");
        // SQLite folds ASCII only; the context compares names without case.
        assert!(!vms.create(&vm("00000000000e", "büro-pc")).await.unwrap(), "nor in other letters' case");
        let mut plain = vm("00000000000d", "Debian");
        plain.settings.nics[0].network = Network::User;
        assert!(vms.create(&plain).await.unwrap());

        assert_eq!(vms.find("0a1b2c3d4e5f").await.unwrap(), Some(first.clone()));
        vms.set_ready("0a1b2c3d4e5f").await.unwrap();
        assert!(vms.set_autostart("0a1b2c3d4e5f", true).await.unwrap());
        let found = vms.find("0a1b2c3d4e5f").await.unwrap().unwrap();
        assert!(found.ready && found.settings.autostart);
        assert_eq!(found.settings.nics[0].network, Network::Bridge("br0".into()));
        assert_eq!(found.settings.boot, [BootDevice::Drive(0), BootDevice::Disk(0)]);
        assert_eq!(vms.find("00000000000d").await.unwrap().unwrap().settings.nics[0].network, Network::User);
        assert_eq!(vms.list().await.unwrap().len(), 2);

        // Changed whole: devices replaced, the name checked.
        let mut changed = found.clone();
        changed.settings.name = "Büro".into();
        changed.settings.cpus = 4;
        changed.settings.disks = vec![Disk {
            slot: 1,
            size_gib: 8,
            bus: DiskBus::Sata,
            location: Location { pool: "cold".into(), volume: 2 },
        }];
        changed.settings.nics[0].network = Network::Direct("enp2s0".into());
        changed.settings.drives = vec![];
        changed.settings.boot = vec![BootDevice::Disk(1)];
        assert!(vms.update(&changed).await.unwrap());
        assert_eq!(vms.find("0a1b2c3d4e5f").await.unwrap(), Some(changed.clone()));
        let taken =
            Vm { settings: VmSettings { name: "debian".into(), ..changed.settings.clone() }, ..changed };
        assert!(!vms.update(&taken).await.unwrap());
        assert!(!vms.update(&vm("00000000000f", "nobody")).await.unwrap(), "no such VM");

        let mut internal = vms.find("00000000000d").await.unwrap().unwrap();
        internal.settings.nics[0].network = Network::Internal("vmnet0".into());
        assert!(vms.update(&internal).await.unwrap());
        assert_eq!(vms.find("00000000000d").await.unwrap(), Some(internal));

        assert!(vms.delete("0a1b2c3d4e5f").await.unwrap());
        assert!(!vms.delete("0a1b2c3d4e5f").await.unwrap());
        assert!(vms.list().await.unwrap().iter().all(|vm| vm.id != "0a1b2c3d4e5f"));
    }

    #[tokio::test]
    async fn a_vm_keeps_its_tpm_secure_boot_and_the_hosts_devices() {
        let (_dir, db) = temporary_database().await;
        let vms = SqliteVms::new(db);
        let mut windows = vm("0a1b2c3d4e5f", "Windows 11");
        assert!(windows.settings.tpm && windows.settings.secure_boot);
        windows.settings.usb = vec![UsbDevice { vendor: 0x046d, product: 0xc31c }];
        windows.settings.pci = vec!["0000:01:00.0".into(), "0000:01:00.1".into()];
        assert!(vms.create(&windows).await.unwrap());
        assert_eq!(vms.find(&windows.id).await.unwrap(), Some(windows.clone()));
        let mut plain = windows.clone();
        plain.settings.tpm = false;
        plain.settings.secure_boot = false;
        plain.settings.usb.clear();
        plain.settings.pci = vec!["0000:02:00.0".into()];
        assert!(vms.update(&plain).await.unwrap());
        assert_eq!(vms.find(&windows.id).await.unwrap(), Some(plain.clone()));
        // Secure Boot needs UEFI, also in the records.
        let mut bios = plain.clone();
        bios.settings.firmware = Firmware::Bios;
        bios.settings.secure_boot = true;
        assert!(vms.update(&bios).await.is_err());
    }

    #[tokio::test]
    async fn snapshot_schedules_and_backups_are_kept_a_backup_also_once_its_vm_went() {
        let (_dir, db) = temporary_database().await;
        let vms = SqliteVms::new(db.clone());
        let schedules = SqliteVmSchedules::new(db.clone());
        let backups = SqliteVmBackups::new(db);
        let office = vm("0a1b2c3d4e5f", "office");
        assert!(vms.create(&office).await.unwrap());
        let daily = SnapshotSchedule { frequency: Frequency::Daily, keep: 7 };
        let hourly = SnapshotSchedule { frequency: Frequency::Hourly, keep: 24 };
        schedules.set(&office.id, &[daily, hourly]).await.unwrap();
        assert_eq!(schedules.list(&office.id).await.unwrap(), [hourly, daily]);
        assert_eq!(schedules.all().await.unwrap()[&office.id], [hourly, daily]);
        schedules.set(&office.id, &[daily]).await.unwrap();
        assert_eq!(schedules.list(&office.id).await.unwrap(), [daily]);

        let new = |taken: i64, place: BackupPlace| NewVmBackup {
            vm: office.id.clone(),
            vm_name: "office".into(),
            place,
            folder: format!("vm-backups/{}/20261112-0{taken}0000", office.id),
            taken_at: Timestamp::from_unix_seconds(1_793_000_000 + taken),
            consistency: Consistency::Crash,
            files: vec![
                BackupFile {
                    part: BackupPart::Disk { slot: 0, volsize: 64 << 30 },
                    bytes: 1 << 30,
                    sha256: "ab".repeat(32),
                },
                BackupFile { part: BackupPart::Nvram, bytes: 540_672, sha256: "cd".repeat(32) },
                BackupFile { part: BackupPart::Tpm, bytes: 1_000, sha256: "ef".repeat(32) },
            ],
            settings: office.settings.clone(),
        };
        let share = BackupPlace::Share { share_id: 3, share: "backups".into(), volume: 1 };
        let first = backups.create(&new(1, share.clone())).await.unwrap();
        let second = backups.create(&new(2, BackupPlace::Drive { drive: "1234-ABCD".into() })).await.unwrap();
        let listed = backups.list().await.unwrap();
        assert_eq!(listed.iter().map(|b| b.id).collect::<Vec<_>>(), [second, first], "newest first");
        let found = backups.find(first).await.unwrap().unwrap();
        assert_eq!(
            (found.place.clone(), found.files.clone(), found.settings.clone()),
            (share, new(1, BackupPlace::Drive { drive: "x".into() }).files, office.settings.clone())
        );
        assert_eq!(found.consistency, Consistency::Crash);
        assert!(vms.delete(&office.id).await.unwrap());
        assert!(schedules.list(&office.id).await.unwrap().is_empty(), "the schedules went with it");
        assert_eq!(backups.list().await.unwrap().len(), 2, "the backups stay");
        assert!(backups.delete(first).await.unwrap() && !backups.delete(first).await.unwrap());
        assert_eq!(backups.find(first).await.unwrap(), None);
    }

    #[tokio::test]
    async fn locations_are_kept_in_the_order_they_were_added() {
        let (_dir, db) = temporary_database().await;
        let locations = SqliteVmLocations::new(db);
        assert!(locations.list().await.unwrap().is_empty());
        let (tank, cold) =
            (Location { pool: "tank".into(), volume: 1 }, Location { pool: "cold".into(), volume: 2 });
        locations.add(&cold).await.unwrap();
        locations.add(&tank).await.unwrap();
        locations.add(&cold).await.unwrap();
        assert_eq!(
            locations.list().await.unwrap(),
            [cold.clone(), tank.clone()],
            "once each, the first first"
        );
        assert!(locations.remove("cold").await.unwrap());
        assert!(!locations.remove("cold").await.unwrap());
        locations.add(&cold).await.unwrap();
        assert_eq!(locations.list().await.unwrap(), [tank, cold]);
    }

    #[tokio::test]
    async fn images_are_kept_with_where_they_came_from_and_stay_while_a_drive_holds_them() {
        let (_dir, db) = temporary_database().await;
        let images = SqliteVmImages::new(db.clone());
        let uploaded = VmImage {
            origin: ImageOrigin::Upload { file: "alpine-virt-3.22.iso".into() },
            bytes: Some(60 << 20),
            ..image("00000000000b", "alpine")
        };
        assert!(images.create(&image("00000000000a", "Win11.iso")).await.unwrap());
        assert!(images.create(&uploaded).await.unwrap());
        assert!(!images.create(&image("00000000000c", "WIN11.ISO")).await.unwrap(), "the name is taken");
        assert_eq!(images.find("00000000000b").await.unwrap(), Some(uploaded));
        images.set_ready("00000000000a", 6 << 30).await.unwrap();
        let ready = images.find("00000000000a").await.unwrap().unwrap();
        assert!(ready.ready && ready.bytes == Some(6 << 30));
        images.set_bytes("00000000000a", 7).await.unwrap();
        assert_eq!(images.find("00000000000a").await.unwrap().unwrap().bytes, Some(7));
        assert!(images.rename("00000000000a", "Windows 11").await.unwrap());
        assert!(!images.rename("00000000000b", "windows 11").await.unwrap());
        assert!(!images.rename("00000000000f", "x").await.unwrap());
        assert_eq!(images.list().await.unwrap().len(), 2);

        let vms = SqliteVms::new(db);
        let mut held = vm("0a1b2c3d4e5f", "web");
        held.settings.drives = vec![Drive { slot: 0, image: Some("00000000000a".into()) }];
        assert!(vms.create(&held).await.unwrap());
        assert!(images.delete("00000000000a").await.is_err(), "the database keeps it too");
        vms.delete("0a1b2c3d4e5f").await.unwrap();
        assert!(images.delete("00000000000a").await.unwrap());
        assert_eq!(images.find("00000000000a").await.unwrap(), None);
    }
}
