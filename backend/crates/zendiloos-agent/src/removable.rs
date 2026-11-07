//! Removable drives: USB sticks and disks, memory cards, mounted for the
//! administrators and ejected again.
//!
//! The agent does not trust zendiloosd's view of the drives. Before it
//! mounts one, and before it powers a disk off, it checks from `lsblk`
//! (the kernel's devices, udev's signatures, the mounts it sees):
//!
//! - exactly one file system has the drive's UUID, and it is one the agent
//!   mounts (FAT, exFAT, NTFS, ext4), directly on the disk or on a
//!   partition, with nothing held on top of it;
//! - its disk is a USB or removable one;
//! - nothing on that disk is a ZFS, RAID, LVM, bcache or LUKS member, or a
//!   device such as an array or a mapping (the system may boot from USB);
//! - nothing on that disk is mounted, used as swap, or held, except the
//!   drive itself at its own place.
//!
//! Mounting and unmounting happen in the host's mount namespace, outside
//! the agent's sandbox, which only sees the result: `systemd-mount` asks
//! systemd for a transient mount unit, which is gone at the next boot and
//! is bound to its device, so systemd unmounts it when the drive is pulled
//! out; `umount` and `rmdir` (a place is removed once nothing is mounted
//! at it) run in transient services (`systemd-run`), and so does powering
//! the disk off, which writes to sysfs (this binary as
//! `power-off-drive <disk>`).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use zendiloos_agent_proto::{DiskName, MEMBER_SIGNATURES, RemovableFilesystem, RemovableId, Response};
use zendiloos_exec::{Limits, Program};

use crate::accounts::{AccountFiles, USERS_GROUP};
use crate::config::Tools;
use crate::operations::OperationError;
use crate::passwd;
use crate::pools::HostPaths;

/// The `lsblk` columns the checks read.
const COLUMNS: &str = "KNAME,TYPE,TRAN,RM,RO,FSTYPE,UUID,MOUNTPOINTS";
/// Mounting replays a journal, unmounting writes what is cached: a slow
/// stick takes minutes for either.
const SLOW: Limits = Limits { timeout: Duration::from_secs(10 * 60), max_stdout: 64 * 1024 };
const QUICK: Limits = Limits { timeout: Duration::from_secs(30), max_stdout: 4 * 1024 * 1024 };

#[derive(Debug)]
pub(crate) struct Removable {
    lsblk: Program,
    systemd_mount: Program,
    systemd_run: Program,
    umount: PathBuf,
    rmdir: PathBuf,
    paths: HostPaths,
    group: PathBuf,
    /// One change of drives at a time.
    serial: tokio::sync::Mutex<()>,
}

/// `lsblk --json --tree`, as far as the checks need it.
#[derive(Debug, Deserialize)]
struct Listing {
    blockdevices: Vec<Node>,
}

#[derive(Debug, Clone, Deserialize)]
struct Node {
    kname: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default, deserialize_with = "flag")]
    rm: bool,
    #[serde(default, deserialize_with = "flag")]
    ro: bool,
    #[serde(default)]
    fstype: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    mountpoints: Vec<Option<String>>,
    #[serde(default)]
    children: Vec<Node>,
}

impl Node {
    fn descendants(&self) -> Vec<&Node> {
        let mut all = vec![self];
        for child in &self.children {
            all.extend(child.descendants());
        }
        all
    }

    fn mounts(&self) -> impl Iterator<Item = &str> {
        self.mountpoints.iter().flatten().map(String::as_str)
    }
}

/// `true`/`false`, or `1`/`0` as older versions of lsblk print flags.
fn flag<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Flag {
        Bool(bool),
        Number(u8),
        Text(String),
    }
    Ok(match Option::<Flag>::deserialize(deserializer)? {
        Some(Flag::Bool(set)) => set,
        Some(Flag::Number(number)) => number != 0,
        Some(Flag::Text(text)) => text == "1",
        None => false,
    })
}

/// A drive that passed the checks of the module documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Drive {
    /// The disk it is on: what is powered off.
    disk: DiskName,
    /// The device carrying the file system: the disk itself or a partition.
    device: String,
    filesystem: RemovableFilesystem,
    read_only: bool,
}

fn refused(reason: String) -> OperationError {
    OperationError::Refused(reason)
}

/// `sda`, `sda1`, `mmcblk0`, `mmcblk0p1`: what reaches `/dev` on a command
/// line.
fn is_device_name(name: &str) -> bool {
    let digits = |s: &str| (1..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit());
    if let Some(rest) = name.strip_prefix("mmcblk") {
        return match rest.split_once('p') {
            Some((disk, partition)) => digits(disk) && digits(partition),
            None => digits(rest),
        };
    }
    let Some(rest) = name.strip_prefix("sd") else { return false };
    let letters = rest.bytes().take_while(u8::is_ascii_lowercase).count();
    let (disk, partition) = rest.split_at(letters);
    (1..=4).contains(&disk.len()) && (partition.is_empty() || digits(partition))
}

/// The checks of the module documentation, against `devices` as lsblk
/// lists them; `place` is the drive's own, where it may be mounted.
fn find(devices: &[Node], id: &RemovableId, place: &str) -> Result<Drive, OperationError> {
    let carrying: Vec<(&Node, &Node)> = devices
        .iter()
        .flat_map(|disk| disk.descendants().into_iter().map(move |node| (disk, node)))
        .filter(|(_, node)| node.uuid.as_deref() == Some(id.as_str()))
        .collect();
    let (disk, device) = match carrying.as_slice() {
        [] => return Err(refused(format!("there is no drive {id}"))),
        [one] => *one,
        many => return Err(refused(format!("{} file systems have the id {id}", many.len()))),
    };
    if disk.kind != "disk" || !(disk.rm || disk.tran.as_deref() == Some("usb")) {
        return Err(refused(format!("{} is no USB or removable disk", disk.kname)));
    }
    let disk_name =
        DiskName::parse(&disk.kname).map_err(|_| refused(format!("{} is no disk name", disk.kname)))?;
    for node in disk.descendants() {
        if let Some(signature) = node.fstype.as_deref().filter(|s| MEMBER_SIGNATURES.contains(s)) {
            return Err(refused(format!("{} is a {signature}: part of a pool, array or volume", node.kname)));
        }
        if node.kind != "disk" && node.kind != "part" {
            return Err(refused(format!("{} holds {} ({})", disk.kname, node.kname, node.kind)));
        }
        let own = std::ptr::eq(node, device);
        if let Some(mount) = node.mounts().find(|&mount| !own || mount != place) {
            return Err(refused(format!("{} is in use at {mount}", node.kname)));
        }
    }
    if !device.children.is_empty() {
        return Err(refused(format!("{} holds other devices", device.kname)));
    }
    let signature = device.fstype.as_deref().unwrap_or("");
    let filesystem = RemovableFilesystem::from_signature(signature)
        .ok_or_else(|| refused(format!("{signature} is no file system for a removable drive")))?;
    if !is_device_name(&device.kname) {
        return Err(refused(format!("{} is no device name", device.kname)));
    }
    Ok(Drive { disk: disk_name, device: device.kname.clone(), filesystem, read_only: device.ro || disk.ro })
}

/// Mount options: no setuid programs, device files or programs from the
/// drive; FAT, exFAT and NTFS have no owners of their own, so everything on
/// them is root's and the group `zendiloos-users`' (`gid`), read and
/// written by them only, executable by nobody. ext4 keeps its owners.
fn options(filesystem: RemovableFilesystem, gid: u32, read_only: bool) -> String {
    let mut options =
        vec!["nosuid".to_owned(), "nodev".to_owned(), "noexec".to_owned(), "noatime".to_owned()];
    options.push(if read_only { "ro" } else { "rw" }.to_owned());
    let owners = [format!("uid=0,gid={gid},fmask=0117,dmask=0007")];
    match filesystem {
        // `flush`: written out early, for drives pulled without ejecting.
        RemovableFilesystem::Fat => {
            options.extend(owners);
            options.extend(["utf8".to_owned(), "shortname=mixed".to_owned(), "flush".to_owned()]);
        }
        RemovableFilesystem::Exfat => options.extend(owners),
        // No names Windows cannot open again.
        RemovableFilesystem::Ntfs => {
            options.extend(owners);
            options.push("windows_names".to_owned());
        }
        RemovableFilesystem::Ext4 => {}
    }
    options.join(",")
}

/// The options of what is mounted at `place`, as `/proc/self/mountinfo`
/// has them (field 5 the mount point, 6 its options); none if nothing is.
fn mounted(mountinfo: &str, place: &str) -> Option<String> {
    mountinfo.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace().skip(4);
        let point = fields.next()?;
        let options = fields.next()?;
        (point == place).then(|| options.to_owned())
    })
}

/// The places with something mounted at them, below `place`: mount point
/// and source device (after the `-` separator: type, source).
fn mounts_below(mountinfo: &str, place: &str) -> Vec<(String, String)> {
    let prefix = format!("{place}/");
    mountinfo
        .lines()
        .filter_map(|line| {
            let point = line.split_whitespace().nth(4)?;
            let rest = line.split_once(" - ")?.1;
            let source = rest.split_whitespace().nth(1)?;
            point.starts_with(&prefix).then(|| (point.to_owned(), source.to_owned()))
        })
        .collect()
}

impl Removable {
    pub(crate) fn new(
        tools: &Tools,
        paths: HostPaths,
        accounts: &AccountFiles,
    ) -> Result<Self, zendiloos_exec::ExecError> {
        Ok(Self {
            lsblk: Program::new(&tools.lsblk)?,
            systemd_mount: Program::new(&tools.systemd_mount)?,
            systemd_run: Program::new(&tools.systemd_run)?,
            umount: tools.umount.clone(),
            rmdir: tools.rmdir.clone(),
            paths,
            group: accounts.group.clone(),
            serial: tokio::sync::Mutex::new(()),
        })
    }

    fn place(&self, id: &RemovableId) -> PathBuf {
        self.paths.removable.join(id.as_str())
    }

    async fn devices(&self) -> Result<Vec<Node>, OperationError> {
        let output =
            zendiloos_exec::run(&self.lsblk, ["--json", "--bytes", "--tree", "--output", COLUMNS], QUICK)
                .await?
                .success(&self.lsblk)?;
        let listing: Listing = serde_json::from_slice(&output.stdout)
            .map_err(|e| OperationError::Failed(format!("lsblk printed what it should not: {e}")))?;
        Ok(listing.blockdevices)
    }

    fn mountinfo(&self) -> Result<String, OperationError> {
        let path = &self.paths.mountinfo;
        fs::read_to_string(path).map_err(|e| OperationError::Read(path.clone(), e))
    }

    pub(crate) async fn mount(&self, id: &RemovableId) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let place = self.place(id);
        let place_text = place.to_string_lossy().into_owned();
        let drive = find(&self.devices().await?, id, &place_text)?;
        if let Some(options) = mounted(&self.mountinfo()?, &place_text) {
            let read_only = options.split(',').any(|option| option == "ro");
            return Ok(Response::RemovableMounted { read_only });
        }
        let group =
            fs::read_to_string(&self.group).map_err(|e| OperationError::Read(self.group.clone(), e))?;
        let gid = passwd::group_id(&group, USERS_GROUP)
            .ok_or_else(|| OperationError::Setup(format!("group {USERS_GROUP} is missing")))?;
        if drive.read_only {
            self.systemd_mount(&drive, id, &place_text, gid, true).await?;
            return Ok(Response::RemovableMounted { read_only: true });
        }
        // Write-protected, or NTFS that Windows left hibernated: at least
        // it can be read.
        if let Err(error) = self.systemd_mount(&drive, id, &place_text, gid, false).await {
            tracing::info!(%error, %id, "the drive cannot be written; mounting it read-only");
            self.systemd_mount(&drive, id, &place_text, gid, true).await.map_err(|_| error)?;
            return Ok(Response::RemovableMounted { read_only: true });
        }
        Ok(Response::RemovableMounted { read_only: false })
    }

    async fn systemd_mount(
        &self,
        drive: &Drive,
        id: &RemovableId,
        place: &str,
        gid: u32,
        read_only: bool,
    ) -> Result<(), OperationError> {
        let kind = format!("--type={}", drive.filesystem.kernel_type());
        let options = format!("--options={}", options(drive.filesystem, gid, read_only));
        let description = format!("--description=zendiloOS: removable drive {id}");
        let device = format!("/dev/{}", drive.device);
        let args = [
            "--no-ask-password",
            "--collect",
            "--fsck=no",
            "--automount=no",
            &kind,
            &options,
            &description,
            "--",
            &device,
            place,
        ];
        zendiloos_exec::run(&self.systemd_mount, args, SLOW).await?.success(&self.systemd_mount)?;
        Ok(())
    }

    pub(crate) async fn eject(&self, id: &RemovableId, power_off: bool) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let place = self.place(id);
        let place_text = place.to_string_lossy().into_owned();
        // Checked as for mounting: nothing else is ever powered off.
        let drive = find(&self.devices().await?, id, &place_text)?;
        if mounted(&self.mountinfo()?, &place_text).is_some() {
            self.unmount(&place_text, false).await?;
        }
        // Never a disk whose file system is still mounted.
        if mounted(&self.mountinfo()?, &place_text).is_some() {
            return Err(OperationError::Busy("the drive is still mounted".to_owned()));
        }
        if place.exists() {
            self.remove_place(&place_text).await?;
        }
        if power_off {
            let agent = self.paths.agent.to_string_lossy().into_owned();
            let unit = format!("--description=zendiloOS: power off {}", drive.disk.as_str());
            let args = [
                "--wait",
                "--pipe",
                "--collect",
                "--quiet",
                &unit,
                &agent,
                "power-off-drive",
                drive.disk.as_str(),
            ];
            zendiloos_exec::run(&self.systemd_run, args, QUICK).await?.success(&self.systemd_run)?;
        }
        Ok(Response::Done)
    }

    /// Removes a place with nothing mounted at it: `rmdir`, so only an
    /// empty folder goes.
    async fn remove_place(&self, place: &str) -> Result<(), OperationError> {
        let rmdir = self.rmdir.to_string_lossy().into_owned();
        let args = ["--wait", "--pipe", "--collect", "--quiet", rmdir.as_str(), "--", place];
        zendiloos_exec::run(&self.systemd_run, args, QUICK).await?.success(&self.systemd_run)?;
        Ok(())
    }

    /// `umount` in the host's namespace; `lazy` detaches it at once and
    /// lets go of it once nothing uses it any more.
    async fn unmount(&self, place: &str, lazy: bool) -> Result<(), OperationError> {
        let umount = self.umount.to_string_lossy().into_owned();
        let mut args = vec!["--wait", "--pipe", "--collect", "--quiet", umount.as_str()];
        if lazy {
            args.push("--lazy");
        }
        args.extend(["--", place]);
        let output = zendiloos_exec::run(&self.systemd_run, &args, SLOW).await?;
        if !output.status.success() && output.stderr.contains("busy") {
            let said = output.stderr.trim().to_owned();
            return Err(OperationError::Busy(format!("files on the drive are still open: {said}")));
        }
        output.success(&self.systemd_run)?;
        Ok(())
    }

    /// Lazily unmounts drives whose device is gone and removes every place
    /// with nothing mounted at it; the ids of those let go of.
    pub(crate) async fn clean(&self) -> Result<Response, OperationError> {
        let _serial = crate::waiting::lock(&self.serial).await?;
        let place = self.paths.removable.to_string_lossy().into_owned();
        let mut ids = Vec::new();
        for (point, source) in mounts_below(&self.mountinfo()?, &place) {
            let gone =
                source.strip_prefix("/dev/").is_none_or(|name| !self.paths.sys_block.join(name).exists());
            if gone {
                self.unmount(&point, true).await?;
            }
        }
        let still = mounts_below(&self.mountinfo()?, &place);
        let entries = match fs::read_dir(&self.paths.removable) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Response::RemovableCleaned { ids });
            }
            Err(error) => return Err(OperationError::Read(self.paths.removable.clone(), error)),
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let text = path.to_string_lossy();
            if still.iter().any(|(point, _)| *point == text) {
                continue;
            }
            let Some(id) = entry.file_name().to_str().and_then(|name| RemovableId::parse(name).ok()) else {
                tracing::warn!(path = %path.display(), "not the place of a removable drive; left alone");
                continue;
            };
            match self.remove_place(&text).await {
                Ok(()) => ids.push(id),
                Err(error) => tracing::warn!(%error, %id, "cannot remove the drive's place"),
            }
        }
        ids.sort();
        Ok(Response::RemovableCleaned { ids })
    }
}

/// `power-off-drive <disk>`, in a transient unit outside the agent's
/// sandbox: the SCSI layer lets go of the disk (writing its cache out and
/// stopping it), then its USB device is removed from the bus, which powers
/// the port off where the hub can; the drive can be pulled out then. Only
/// for a disk on USB or a removable one.
pub fn power_off_drive(paths: &HostPaths, disk: &DiskName) -> Result<(), String> {
    let block = paths.sys_block.join(disk.as_str());
    let removable = fs::read_to_string(block.join("removable")).is_ok_and(|text| text.trim() == "1");
    let device = fs::canonicalize(block.join("device"))
        .map_err(|e| format!("{}: no device of the disk: {e}", disk.as_str()))?;
    let usb = device
        .ancestors()
        .find(|dir| dir.join("idVendor").exists() && dir.join("remove").exists())
        .map(Path::to_owned);
    if usb.is_none() && !removable {
        return Err(format!("{} is no USB or removable disk", disk.as_str()));
    }
    let delete = device.join("delete");
    if delete.exists() {
        fs::write(&delete, "1").map_err(|e| format!("{}: {e}", delete.display()))?;
    }
    if let Some(usb) = usb {
        let remove = usb.join("remove");
        fs::write(&remove, "1").map_err(|e| format!("{}: {e}", remove.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn listing(value: serde_json::Value) -> Vec<Node> {
        serde_json::from_value::<Listing>(json!({ "blockdevices": value })).unwrap().blockdevices
    }

    fn stick(partition: serde_json::Value) -> serde_json::Value {
        json!({ "kname": "sdf", "type": "disk", "tran": "usb", "rm": true, "ro": false,
                "fstype": null, "uuid": null, "mountpoints": [null], "children": [partition] })
    }

    fn fat(mount: Option<&str>) -> serde_json::Value {
        json!({ "kname": "sdf1", "type": "part", "tran": null, "rm": true, "ro": false,
                "fstype": "vfat", "uuid": "1A2B-3C4D", "mountpoints": [mount] })
    }

    const PLACE: &str = "/run/zendiloos-removable/1A2B-3C4D";

    fn id() -> RemovableId {
        RemovableId::parse("1A2B-3C4D").unwrap()
    }

    fn reason(result: Result<Drive, OperationError>) -> String {
        match result {
            Err(OperationError::Refused(reason)) => reason,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn a_stick_with_fat_is_found_unmounted_or_at_its_place() {
        for mount in [None, Some(PLACE)] {
            let drive = find(&listing(json!([stick(fat(mount))])), &id(), PLACE).unwrap();
            assert_eq!(drive.disk.as_str(), "sdf");
            assert_eq!(drive.device, "sdf1");
            assert_eq!(drive.filesystem, RemovableFilesystem::Fat);
            assert!(!drive.read_only);
        }
        // A whole disk with a file system and no partitions, flags as 0/1.
        let whole = json!({ "kname": "sdg", "type": "disk", "tran": "usb", "rm": "0", "ro": 1,
                            "fstype": "exfat", "uuid": "1A2B-3C4D", "mountpoints": [null] });
        let drive = find(&listing(json!([whole])), &id(), PLACE).unwrap();
        assert_eq!((drive.device.as_str(), drive.read_only), ("sdg", true));
    }

    #[test]
    fn what_is_not_a_plain_removable_drive_is_refused() {
        let at = |value| reason(find(&listing(value), &id(), PLACE));
        assert!(at(json!([])).contains("no drive"));
        // A SATA disk.
        let mut sata = stick(fat(None));
        sata["tran"] = json!("sata");
        sata["rm"] = json!(false);
        assert!(at(json!([sata])).contains("no USB or removable disk"));
        // Mounted somewhere else, the system on a USB disk.
        assert!(at(json!([stick(fat(Some("/media/stick")))])).contains("in use at /media/stick"));
        let mut system = stick(fat(None));
        system["children"].as_array_mut().unwrap().push(
            json!({ "kname": "sdf2", "type": "part", "fstype": "ext4", "uuid": "r", "mountpoints": ["/"] }),
        );
        assert!(at(json!([system])).contains("in use at /"));
        // Another part of the disk belongs to a pool, an array or a mapping.
        for (kind, fstype) in [("part", "zfs_member"), ("part", "linux_raid_member"), ("part", "crypto_LUKS")]
        {
            let mut member = stick(fat(None));
            member["children"].as_array_mut().unwrap().push(
                json!({ "kname": "sdf2", "type": kind, "fstype": fstype, "uuid": "x", "mountpoints": [null] }),
            );
            assert!(at(json!([member])).contains(fstype), "{fstype}");
        }
        let mut held = fat(None);
        held["children"] = json!([{ "kname": "dm-0", "type": "crypt", "mountpoints": [null] }]);
        assert!(at(json!([stick(held)])).contains("dm-0"));
        // A file system that is not for removable drives.
        let mut btrfs = fat(None);
        btrfs["fstype"] = json!("btrfs");
        assert!(at(json!([stick(btrfs)])).contains("btrfs is no file system"));
        // Two with the same UUID: a cloned stick.
        let mut clone = stick(fat(None));
        clone["kname"] = json!("sdg");
        clone["children"][0]["kname"] = json!("sdg1");
        assert!(at(json!([stick(fat(None)), clone])).contains("2 file systems"));
        // A name that would not stay a name on a command line.
        let mut odd = fat(None);
        odd["kname"] = json!("sdf1 -o");
        assert!(at(json!([stick(odd)])).contains("no device name"));
    }

    #[test]
    fn device_names_are_disks_and_partitions_only() {
        for name in ["sda", "sdab1", "sdf12", "mmcblk0", "mmcblk1p1"] {
            assert!(is_device_name(name), "{name}");
        }
        for name in ["", "sd", "sda1b", "nvme0n1", "mmcblk", "mmcblkp1", "../sda", "sda 1", "sdaaaaa"] {
            assert!(!is_device_name(name), "{name}");
        }
    }

    #[test]
    fn options_keep_programs_and_owners_off_the_drive() {
        let fat = options(RemovableFilesystem::Fat, 990, false);
        assert_eq!(
            fat,
            "nosuid,nodev,noexec,noatime,rw,uid=0,gid=990,fmask=0117,dmask=0007,utf8,shortname=mixed,flush"
        );
        assert!(options(RemovableFilesystem::Ntfs, 990, true).contains(",ro,uid=0,gid=990,"));
        assert!(options(RemovableFilesystem::Ntfs, 990, true).ends_with("windows_names"));
        assert_eq!(options(RemovableFilesystem::Ext4, 990, false), "nosuid,nodev,noexec,noatime,rw");
        assert!(options(RemovableFilesystem::Exfat, 7, false).ends_with("gid=7,fmask=0117,dmask=0007"));
    }

    #[test]
    fn mountinfo_tells_what_is_mounted_where() {
        let mountinfo = "22 1 0:21 / / rw,relatime - ext4 /dev/vda2 rw\n\
             90 22 8:81 / /run/zendiloos-removable/1A2B-3C4D rw,nosuid,nodev,noexec - vfat /dev/sdf1 rw\n\
             91 22 8:97 / /run/zendiloos-removable/9F ro,nosuid - ntfs3 /dev/sdg1 ro\n";
        assert_eq!(mounted(mountinfo, PLACE).as_deref(), Some("rw,nosuid,nodev,noexec"));
        assert_eq!(mounted(mountinfo, "/run/zendiloos-removable/9F").as_deref(), Some("ro,nosuid"));
        assert_eq!(mounted(mountinfo, "/run/zendiloos-removable/1A2B"), None);
        assert_eq!(
            mounts_below(mountinfo, "/run/zendiloos-removable"),
            [
                (PLACE.to_owned(), "/dev/sdf1".to_owned()),
                ("/run/zendiloos-removable/9F".to_owned(), "/dev/sdg1".to_owned())
            ]
        );
    }

    /// A sysfs tree: the disk's device below a USB device.
    fn sysfs(root: &Path, usb: bool, removable: bool) -> HostPaths {
        let devices = root.join("devices/usb2/2-1");
        let scsi = devices.join("2-1:1.0/host6/target6:0:0/6:0:0:0");
        fs::create_dir_all(&scsi).unwrap();
        fs::write(scsi.join("delete"), "").unwrap();
        if usb {
            fs::write(devices.join("idVendor"), "0781\n").unwrap();
            fs::write(devices.join("remove"), "").unwrap();
        }
        let block = root.join("class-block/sdf");
        fs::create_dir_all(&block).unwrap();
        fs::write(block.join("removable"), if removable { "1\n" } else { "0\n" }).unwrap();
        std::os::unix::fs::symlink(&scsi, block.join("device")).unwrap();
        HostPaths { sys_block: root.join("class-block"), ..HostPaths::default() }
    }

    #[test]
    fn powering_off_lets_go_of_the_disk_then_of_its_usb_device() {
        let root = tempfile::tempdir().unwrap();
        let paths = sysfs(root.path(), true, false);
        power_off_drive(&paths, &DiskName::parse("sdf").unwrap()).unwrap();
        let scsi = root.path().join("devices/usb2/2-1/2-1:1.0/host6/target6:0:0/6:0:0:0");
        assert_eq!(fs::read_to_string(scsi.join("delete")).unwrap(), "1");
        assert_eq!(fs::read_to_string(root.path().join("devices/usb2/2-1/remove")).unwrap(), "1");
    }

    #[test]
    fn only_usb_or_removable_disks_are_powered_off() {
        let root = tempfile::tempdir().unwrap();
        let paths = sysfs(root.path(), false, false);
        let error = power_off_drive(&paths, &DiskName::parse("sdf").unwrap()).unwrap_err();
        assert!(error.contains("no USB or removable disk"), "{error}");
        let scsi = root.path().join("devices/usb2/2-1/2-1:1.0/host6/target6:0:0/6:0:0:0");
        assert_eq!(fs::read_to_string(scsi.join("delete")).unwrap(), "", "untouched");
        // A card reader's disk: removable, no USB device above it.
        let reader = tempfile::tempdir().unwrap();
        let paths = sysfs(reader.path(), false, true);
        power_off_drive(&paths, &DiskName::parse("sdf").unwrap()).unwrap();
        let missing = power_off_drive(&paths, &DiskName::parse("sdz").unwrap()).unwrap_err();
        assert!(missing.contains("no device"), "{missing}");
    }
}
