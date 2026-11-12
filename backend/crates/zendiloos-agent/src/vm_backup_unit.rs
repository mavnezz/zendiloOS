//! Backups of virtual machines and restoring them (docs/architecture.md,
//! 4.13): the work of `back-up-vm`, `restore-vm` and `remove-vm-backup`,
//! which the system agent starts in transient units, each in a sandbox of
//! its own that may write only where its work lands and open no device but
//! the VM's zvols.
//!
//! A backup reads each disk whole from a snapshot's device and writes it
//! as a raw image compressed with zstd (`disk<N>.raw.zst`): any machine
//! reads it back with Debian's `zstd`, it goes onto a zvol of any pool or
//! into another hypervisor, and a restore never feeds a ZFS stream from a
//! share that users may write to the kernel's `zfs receive`, only bytes
//! into the guest's own disk. Zeros, what a sparse zvol mostly holds,
//! cost almost nothing compressed. The UEFI variables and the TPM's state
//! come along as they are, and a manifest with zendiloOS's settings of the
//! VM and every file's SHA-256 last. All are written under hidden names and
//! made writable by root only, then put in place, the manifest last, as the
//! volumes' backups are ([`crate::volume_archive`]).
//!
//! A restore opens each file beneath the share's root without following a
//! link, refuses one that others may write, and hashes what it reads: a
//! file that is not the one recorded fails the restore, which the system
//! agent then undoes. Runs of zeros are punched as holes, so a disk stays
//! sparse, and what lies after the image on the zvol is punched too.

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use rustix::fs::{AtFlags, FallocateFlags, FileType, OFlags, RenameFlags, fstat, fsync};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zendiloos_agent_proto::{
    ArchiveDigest, SharePath, VM_BACKUP_MANIFEST, VmBackupFile, VmBackupPart, VmConsistency, WrittenFile,
};

use crate::volume_archive::{
    ArchiveError, Hashing, Partial, Progress, UnitLine, beneath, decode, discard, failed, identity, kind,
    make_folders, modified, open_archive, open_folder, refused, say, sweep_partials, write_partial,
};

/// How hard zstd works: Debian's default, fast and already a good ratio.
const LEVEL: i32 = 3;
/// What is read from a disk at once.
const CHUNK: usize = 1024 * 1024;
/// What a run of zeros is looked for in: a zvol's usual block.
const BLOCK: usize = 64 * 1024;
/// Where a manifest says which format it has.
const MANIFEST_FORMAT: u32 = 1;
/// The UEFI variables and a TPM's state are small: more is no such file.
const MAX_SMALL_FILE: u64 = 64 * 1024 * 1024;

/// A disk to back up: its slot, the snapshot's device and its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackupDisk {
    pub slot: zendiloos_agent_proto::VmDiskSlot,
    pub device: PathBuf,
    pub volsize: u64,
}

/// What the system agent asks of a backup's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VmBackupJob {
    /// The share's or the drive's root.
    pub root: PathBuf,
    /// The backup's folder beneath it: `<folder>/<id>/<stem>`.
    pub folder: SharePath,
    pub vm: String,
    pub disks: Vec<BackupDisk>,
    /// The UEFI variables, if it has any.
    pub nvram: Option<PathBuf>,
    /// The TPM's state, if it has one.
    pub tpm: Option<PathBuf>,
    /// zendiloOS's settings of the VM, a JSON object.
    pub settings: String,
    pub consistency: VmConsistency,
}

/// What a backup wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VmBackupDone {
    pub files: Vec<VmBackupFile>,
    pub manifest: WrittenFile,
}

/// The manifest beside a VM's files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct VmManifest {
    pub format: u32,
    pub vm: String,
    /// zendiloOS's settings of the VM.
    pub settings: serde_json::Value,
    pub consistency: VmConsistency,
    /// Unix seconds.
    pub backed_up_at: i64,
    pub zendiloos_version: String,
    pub files: Vec<VmBackupFile>,
}

/// A file to restore, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreFile {
    pub file: VmBackupFile,
    /// A disk's zvol device, or the file the UEFI variables or the TPM's
    /// state go to.
    pub target: PathBuf,
    /// Whose the file and the folders made for it are: swtpm's account
    /// for a TPM's state; root's when none.
    #[serde(default)]
    pub owner: Option<(u32, u32)>,
}

/// What the system agent asks of a restore's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VmRestoreJob {
    pub root: PathBuf,
    /// The backup's folder beneath it.
    pub folder: SharePath,
    /// The disks first, then the small files.
    pub files: Vec<RestoreFile>,
}

/// What the system agent asks of the unit that removes a backup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VmRemoveJob {
    pub root: PathBuf,
    pub folder: SharePath,
    pub files: Vec<VmBackupPart>,
}

/// Which job a unit runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmBackupWork {
    BackUp,
    Restore,
    Remove,
}

impl VmBackupWork {
    /// The command of this binary that runs it.
    pub(crate) fn command(self) -> &'static str {
        match self {
            Self::BackUp => "back-up-vm",
            Self::Restore => "restore-vm",
            Self::Remove => "remove-vm-backup",
        }
    }
}

/// A disk's device or a zvol's: opened by the path the agent made of
/// checked parts, through the links udev made, and only a block device (a
/// plain file in tests, where no device is).
fn open_device(path: &std::path::Path, write: bool) -> Result<(File, u64), ArchiveError> {
    let real = std::fs::canonicalize(path).map_err(|e| failed(path.display(), e))?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .open(&real)
        .map_err(|e| failed(real.display(), e))?;
    let stat = fstat(&file).map_err(|e| failed(real.display(), e))?;
    if !matches!(kind(&stat), FileType::BlockDevice | FileType::RegularFile) {
        return Err(refused(format!("{} is no disk", real.display())));
    }
    let size = file.seek(SeekFrom::End(0)).map_err(|e| failed(real.display(), e))?;
    file.seek(SeekFrom::Start(0)).map_err(|e| failed(real.display(), e))?;
    Ok((file, size))
}

/// Writes `source` (of `size` bytes) compressed into `out`; its SHA-256 and
/// the bytes written.
fn compress(
    source: &mut impl Read,
    size: u64,
    out: &File,
    progress: &mut Progress<'_>,
) -> Result<(String, u64), ArchiveError> {
    let hashing = Hashing::new(BufWriter::with_capacity(CHUNK, out));
    let mut encoder = zstd::stream::write::Encoder::new(hashing, LEVEL).map_err(|e| failed("zstd", e))?;
    let mut buffer = vec![0u8; CHUNK];
    let mut read = 0u64;
    while read < size {
        let n = source.read(&mut buffer).map_err(|e| failed("the disk", e))?;
        if n == 0 {
            break;
        }
        encoder.write_all(&buffer[..n]).map_err(|e| failed("the image", e))?;
        read += n as u64;
        progress.add(n as u64);
    }
    if read != size {
        return Err(ArchiveError::Failed(format!("the disk ended after {read} of {size} bytes")));
    }
    let mut hashing = encoder.finish().map_err(|e| failed("zstd", e))?;
    hashing.flush().map_err(|e| failed("the image", e))?;
    Ok((hashing.digest(), hashing.bytes))
}

/// A small file read whole, at most [`MAX_SMALL_FILE`]; none if it is not
/// there.
fn small_file(path: &std::path::Path) -> Result<Option<Vec<u8>>, ArchiveError> {
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(OFlags::NOFOLLOW.bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failed(path.display(), error)),
    };
    let stat = fstat(&file).map_err(|e| failed(path.display(), e))?;
    if kind(&stat) != FileType::RegularFile {
        return Err(refused(format!("{} is no file", path.display())));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SMALL_FILE + 1).read_to_end(&mut bytes).map_err(|e| failed(path.display(), e))?;
    if bytes.len() as u64 > MAX_SMALL_FILE {
        return Err(refused(format!("{} is too large", path.display())));
    }
    Ok(Some(bytes))
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(text: &str) -> Result<ArchiveDigest, ArchiveError> {
    ArchiveDigest::parse(text).map_err(|e| failed("the digest", e))
}

/// Backs up a VM as `job` says. Whatever fails, nothing it wrote stays:
/// its files are complete before any is put in place, the manifest last.
pub(crate) fn back_up(
    job: &VmBackupJob,
    now: i64,
    report: &mut dyn FnMut(u8),
) -> Result<VmBackupDone, ArchiveError> {
    let settings: serde_json::Value =
        serde_json::from_str(&job.settings).map_err(|_| refused("the VM's settings are no JSON"))?;
    let root = open_folder(&job.root, OFlags::PATH)?;
    let target = make_folders(root.as_fd(), &job.folder)?;
    let swept = sweep_partials(target.as_fd());
    if swept > 0 {
        tracing::info!(swept, "hidden files earlier backups left are removed");
    }
    let total: u64 = job.disks.iter().map(|disk| disk.volsize).sum();
    let mut progress = Progress::new(total, report);
    let mut written: Vec<(Partial, VmBackupFile)> = Vec::new();
    let outcome = (|| {
        for disk in &job.disks {
            let (mut device, size) = open_device(&disk.device, false)?;
            if size != disk.volsize {
                return Err(ArchiveError::Failed(format!(
                    "disk {} has {size} bytes, not the {} it should",
                    disk.slot.get(),
                    disk.volsize
                )));
            }
            let part = VmBackupPart::Disk { slot: disk.slot, volsize: disk.volsize };
            let mut result = None;
            let partial = write_partial(target.as_fd(), &part.file_name(), |file| {
                result = Some(compress(&mut device, size, file, &mut progress)?);
                Ok(())
            })?;
            let (sha, bytes) = result.ok_or_else(|| ArchiveError::Failed("nothing written".into()))?;
            written.push((partial, VmBackupFile { part, bytes, sha256: digest(&sha)? }));
        }
        for (part, source) in [(VmBackupPart::Nvram, &job.nvram), (VmBackupPart::Tpm, &job.tpm)] {
            let Some(source) = source else { continue };
            let Some(bytes) = small_file(source)? else { continue };
            let partial = write_partial(target.as_fd(), &part.file_name(), |mut file| {
                file.write_all(&bytes).map_err(|e| failed(part.file_name(), e))
            })?;
            written.push((
                partial,
                VmBackupFile { part, bytes: bytes.len() as u64, sha256: digest(&sha256(&bytes))? },
            ));
        }
        let files: Vec<VmBackupFile> = written.iter().map(|(_, file)| file.clone()).collect();
        let manifest = VmManifest {
            format: MANIFEST_FORMAT,
            vm: job.vm.clone(),
            settings,
            consistency: job.consistency,
            backed_up_at: now,
            zendiloos_version: crate::VERSION.to_owned(),
            files: files.clone(),
        };
        let json = serde_json::to_string_pretty(&manifest).map_err(|e| failed("the manifest", e))? + "\n";
        let manifest = write_partial(target.as_fd(), VM_BACKUP_MANIFEST, |mut file| {
            file.write_all(json.as_bytes()).map_err(|e| failed(VM_BACKUP_MANIFEST, e))
        })?;
        Ok((files, manifest))
    })();
    let (files, manifest) = match outcome {
        Ok(done) => done,
        Err(error) => {
            for (partial, _) in &written {
                discard(target.as_fd(), partial);
            }
            return Err(error);
        }
    };
    // In place, the manifest last; should one not go, those placed go again.
    let mut placed: Vec<(String, (u64, u64))> = Vec::new();
    let mut all: Vec<&Partial> = written.iter().map(|(partial, _)| partial).collect();
    all.push(&manifest);
    for partial in &all {
        let id = fstat(&partial.file).map(|stat| identity(&stat)).map_err(|e| failed(&partial.name, e));
        let renamed = id.and_then(|id| {
            rustix::fs::renameat_with(
                target.as_fd(),
                partial.hidden.as_str(),
                target.as_fd(),
                partial.name.as_str(),
                RenameFlags::NOREPLACE,
            )
            .map(|()| id)
            .map_err(|e| failed(&partial.name, e))
        });
        match renamed {
            Ok(id) => placed.push((partial.name.clone(), id)),
            Err(error) => {
                for (name, id) in &placed {
                    crate::volume_archive::remove_if(target.as_fd(), name, *id);
                }
                for partial in &all {
                    discard(target.as_fd(), partial);
                }
                return Err(error);
            }
        }
    }
    let stat = fstat(&manifest.file).map_err(|e| failed(VM_BACKUP_MANIFEST, e))?;
    let (device, inode) = identity(&stat);
    let path = job.folder.join(VM_BACKUP_MANIFEST).map_err(|_| refused("the folder's path is too long"))?;
    Ok(VmBackupDone {
        files,
        manifest: WrittenFile {
            path,
            bytes: u64::try_from(stat.st_size).unwrap_or(0),
            device,
            inode,
            modified: modified(&stat),
        },
    })
}

/// Punches `len` bytes at `offset` out of `target`, or writes zeros where
/// it cannot: the zvol frees them.
fn zeros(target: &File, offset: u64, len: u64) -> Result<(), ArchiveError> {
    if len == 0 {
        return Ok(());
    }
    let punched =
        rustix::fs::fallocate(target, FallocateFlags::PUNCH_HOLE | FallocateFlags::KEEP_SIZE, offset, len);
    if punched.is_ok() {
        return Ok(());
    }
    let block = vec![0u8; BLOCK];
    let mut done = 0;
    while done < len {
        let n = (len - done).min(BLOCK as u64) as usize;
        target.write_all_at(&block[..n], offset + done).map_err(|e| failed("the disk", e))?;
        done += n as u64;
    }
    Ok(())
}

/// Writes the image of `file` from `folder` onto the device `target`:
/// zeros as holes, the rest written, what lies after it punched; refused
/// unless what was read is the file recorded, and the image the disk's
/// size or less.
fn restore_disk(
    folder: std::os::fd::BorrowedFd<'_>,
    folder_path: &SharePath,
    file: &VmBackupFile,
    target: &std::path::Path,
    progress: &mut Progress<'_>,
) -> Result<(), ArchiveError> {
    let VmBackupPart::Disk { slot, volsize } = file.part else {
        return Err(refused("no disk"));
    };
    let path =
        folder_path.join(&file.part.file_name()).map_err(|_| refused("the folder's path is too long"))?;
    let (archive, bytes) = open_archive(folder, &path)?;
    if bytes != file.bytes {
        return Err(refused(format!("{} has {bytes} bytes, not the {} recorded", path.as_str(), file.bytes)));
    }
    let (device, size) = open_device(target, true)?;
    if size < volsize {
        return Err(ArchiveError::Failed(format!(
            "disk {} has {size} bytes, less than the image's {volsize}",
            slot.get()
        )));
    }
    let mut hashing = Hashing::new(archive);
    let mut decoder = zstd::stream::read::Decoder::new(&mut hashing).map_err(|e| failed("zstd", e))?;
    let mut buffer = vec![0u8; CHUNK];
    let mut offset = 0u64;
    let mut hole: Option<u64> = None;
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            let n = decoder.read(&mut buffer[filled..]).map_err(|e| failed(path.as_str(), e))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        if offset + filled as u64 > volsize {
            return Err(refused(format!("{} holds more than its disk's {volsize} bytes", path.as_str())));
        }
        for block in buffer[..filled].chunks(BLOCK) {
            if block.iter().all(|b| *b == 0) {
                hole.get_or_insert(offset);
            } else {
                if let Some(start) = hole.take() {
                    zeros(&device, start, offset - start)?;
                }
                device.write_all_at(block, offset).map_err(|e| failed("the disk", e))?;
            }
            offset += block.len() as u64;
        }
        progress.add(filled as u64);
    }
    drop(decoder);
    if offset != volsize {
        return Err(refused(format!("{} holds {offset} bytes, not its disk's {volsize}", path.as_str())));
    }
    if hashing.digest() != file.sha256.as_str() || hashing.bytes != file.bytes {
        return Err(refused(format!(
            "{} is not the file the backup wrote: its SHA-256 differs",
            path.as_str()
        )));
    }
    let start = hole.unwrap_or(offset);
    zeros(&device, start, size - start)?;
    fsync(&device).map_err(|e| failed("the disk", e))?;
    Ok(())
}

/// Writes a small file of the backup to `target`, root's alone, under a
/// hidden name first; refused unless it is the file recorded.
fn restore_small(
    folder: std::os::fd::BorrowedFd<'_>,
    folder_path: &SharePath,
    file: &VmBackupFile,
    target: &std::path::Path,
    owner: Option<(u32, u32)>,
) -> Result<(), ArchiveError> {
    let path =
        folder_path.join(&file.part.file_name()).map_err(|_| refused("the folder's path is too long"))?;
    let (archive, bytes) = open_archive(folder, &path)?;
    if bytes != file.bytes || bytes > MAX_SMALL_FILE {
        return Err(refused(format!("{} has {bytes} bytes, not the {} recorded", path.as_str(), file.bytes)));
    }
    let mut content = Vec::new();
    archive.take(MAX_SMALL_FILE).read_to_end(&mut content).map_err(|e| failed(path.as_str(), e))?;
    if sha256(&content) != file.sha256.as_str() {
        return Err(refused(format!(
            "{} is not the file the backup wrote: its SHA-256 differs",
            path.as_str()
        )));
    }
    let parent = target.parent().ok_or_else(|| refused("no folder"))?;
    // The folders made for it: the TPM's `<uuid>/tpm2`.
    let made: Vec<PathBuf> =
        parent.ancestors().take(2).filter(|folder| !folder.exists()).map(std::path::Path::to_owned).collect();
    std::fs::create_dir_all(parent).map_err(|e| failed(parent.display(), e))?;
    let hidden = parent.join(format!(
        ".{}.restore",
        target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let written = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(OFlags::NOFOLLOW.bits() as i32)
            .open(&hidden)?;
        out.write_all(&content)?;
        out.sync_all()?;
        std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&hidden, target)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&hidden);
        return Err(failed(target.display(), error));
    }
    if let Some((uid, gid)) = owner {
        for path in made.iter().map(PathBuf::as_path).chain([target]) {
            if let Err(error) = std::os::unix::fs::chown(path, Some(uid), Some(gid)) {
                tracing::warn!(%error, path = %path.display(), "cannot give the restored file to its account");
            }
        }
    }
    Ok(())
}

/// Restores a VM's files as `job` says: the disks first, then the small
/// files, each checked against what the backup recorded.
pub(crate) fn restore(
    job: &VmRestoreJob,
    report: &mut dyn FnMut(u8),
) -> Result<serde_json::Value, ArchiveError> {
    let root = open_folder(&job.root, OFlags::PATH)?;
    let folder = beneath(root.as_fd(), b"", OFlags::PATH | OFlags::DIRECTORY)
        .map_err(|e| failed("the backup's root", e))?;
    let total: u64 = job
        .files
        .iter()
        .filter_map(|file| match file.file.part {
            VmBackupPart::Disk { volsize, .. } => Some(volsize),
            _ => None,
        })
        .sum();
    let mut progress = Progress::new(total, report);
    for file in job.files.iter().filter(|file| matches!(file.file.part, VmBackupPart::Disk { .. })) {
        restore_disk(folder.as_fd(), &job.folder, &file.file, &file.target, &mut progress)?;
    }
    for file in job.files.iter().filter(|file| !matches!(file.file.part, VmBackupPart::Disk { .. })) {
        restore_small(folder.as_fd(), &job.folder, &file.file, &file.target, file.owner)?;
    }
    Ok(serde_json::json!({ "restored": job.files.len() }))
}

/// Removes a backup's files, by their names in its folder only, and the
/// folder and the VM's above it once empty. Whether anything went.
pub(crate) fn remove(job: &VmRemoveJob) -> Result<bool, ArchiveError> {
    let root = open_folder(&job.root, OFlags::PATH)?;
    let folder = match beneath(root.as_fd(), job.folder.as_str().as_bytes(), OFlags::PATH | OFlags::DIRECTORY)
    {
        Ok(folder) => folder,
        Err(rustix::io::Errno::NOENT) => return Ok(false),
        Err(error) => return Err(crate::volume_archive::link_on_the_way(job.folder.as_str(), error)),
    };
    let mut removed = false;
    let mut names: Vec<String> = job.files.iter().map(|part| part.file_name()).collect();
    names.push(VM_BACKUP_MANIFEST.to_owned());
    for name in names {
        match rustix::fs::statat(&folder, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if kind(&stat) == FileType::RegularFile => {
                rustix::fs::unlinkat(&folder, name.as_str(), AtFlags::empty())
                    .map_err(|e| failed(&name, e))?;
                removed = true;
            }
            _ => {}
        }
    }
    drop(folder);
    // The backup's folder, then the VM's, each only once empty.
    let mut path = job.folder.as_str().to_owned();
    for _ in 0..2 {
        if path.is_empty() {
            break;
        }
        let (parent, name) = path
            .rsplit_once('/')
            .map_or((String::new(), path.clone()), |(parent, name)| (parent.to_owned(), name.to_owned()));
        let parent_fd = beneath(root.as_fd(), parent.as_bytes(), OFlags::PATH | OFlags::DIRECTORY);
        let Ok(parent_fd) = parent_fd else { break };
        if rustix::fs::unlinkat(&parent_fd, name.as_str(), AtFlags::REMOVEDIR).is_err() {
            break;
        }
        path = parent;
    }
    Ok(removed)
}

/// Runs a unit's job, printing [`UnitLine`]s to `out`; whether it was done.
pub fn run_vm_backup_job(work: VmBackupWork, hex: &str, now: i64, out: &mut dyn Write) -> bool {
    let outcome: Result<serde_json::Value, ArchiveError> = {
        let mut progress = |step: &str, percent: u8| {
            say(out, &UnitLine::Progress { step: step.to_owned(), percent });
        };
        match work {
            VmBackupWork::BackUp => {
                decode::<VmBackupJob>(hex).map_err(ArchiveError::Refused).and_then(|job| {
                    back_up(&job, now, &mut |percent| progress("backing_up", percent))
                        .and_then(|done| serde_json::to_value(done).map_err(|e| failed("the result", e)))
                })
            }
            VmBackupWork::Restore => decode::<VmRestoreJob>(hex)
                .map_err(ArchiveError::Refused)
                .and_then(|job| restore(&job, &mut |percent| progress("restoring", percent))),
            VmBackupWork::Remove => decode::<VmRemoveJob>(hex)
                .map_err(ArchiveError::Refused)
                .and_then(|job| remove(&job).map(serde_json::Value::Bool)),
        }
    };
    match outcome {
        Ok(value) => {
            crate::volume_archive::say_value(out, value);
            true
        }
        Err(error) => {
            let refused = matches!(error, ArchiveError::Refused(_));
            say(out, &UnitLine::Failed { message: error.to_string(), refused });
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use zendiloos_agent_proto::VmDiskSlot;

    use super::*;

    /// A 4 MiB disk with data at its start, in its middle and at its end,
    /// zeros between; the share and the libvirt folders in a temporary root.
    fn disk(dir: &std::path::Path, name: &str) -> (PathBuf, Vec<u8>) {
        let mut data = vec![0u8; 4 << 20];
        data[..5].copy_from_slice(b"hello");
        data[2 << 20..(2 << 20) + 4].copy_from_slice(b"mid!");
        let end = data.len() - 3;
        data[end..].copy_from_slice(b"end");
        let path = dir.join(name);
        std::fs::write(&path, &data).unwrap();
        (path, data)
    }

    fn job(dir: &std::path::Path) -> VmBackupJob {
        std::fs::create_dir_all(dir.join("share")).unwrap();
        std::fs::write(dir.join("nvram"), b"vars").unwrap();
        VmBackupJob {
            root: dir.join("share"),
            folder: SharePath::parse("backups/vms/0a1b2c3d4e5f/20261112-030000").unwrap(),
            vm: "0a1b2c3d4e5f".into(),
            disks: vec![BackupDisk {
                slot: VmDiskSlot::new(0).unwrap(),
                device: dir.join("disk0"),
                volsize: 4 << 20,
            }],
            nvram: Some(dir.join("nvram")),
            tpm: Some(dir.join("no-tpm")),
            settings: r#"{"name":"web","cpus":2}"#.into(),
            consistency: VmConsistency::Crash,
        }
    }

    #[test]
    fn a_backup_writes_each_disk_compressed_the_variables_and_a_manifest_last() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (_, data) = disk(&root, "disk0");
        let job = job(&root);
        let mut told = Vec::new();
        let done = back_up(&job, 1_793_000_000, &mut |percent| told.push(percent)).unwrap();
        assert_eq!(told.last(), Some(&100));
        let folder = root.join("share/backups/vms/0a1b2c3d4e5f/20261112-030000");
        let mut names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["disk0.raw.zst", "manifest.json", "nvram.fd"], "no TPM, no hidden file left");
        let image = std::fs::read(folder.join("disk0.raw.zst")).unwrap();
        assert!(image.len() < 64 * 1024, "zeros cost almost nothing: {}", image.len());
        assert_eq!(zstd::decode_all(&image[..]).unwrap(), data, "Debian's zstd reads it back");
        let mode = std::fs::metadata(folder.join("disk0.raw.zst")).unwrap().permissions().mode();
        assert_eq!(mode & 0o022, 0, "writable by root only");
        assert_eq!(done.files.len(), 2);
        assert_eq!(done.files[0].sha256.as_str(), sha256(&image));
        assert_eq!(
            done.files[1],
            VmBackupFile { part: VmBackupPart::Nvram, bytes: 4, sha256: digest(&sha256(b"vars")).unwrap() }
        );
        let manifest: VmManifest =
            serde_json::from_slice(&std::fs::read(folder.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.settings["name"], "web");
        assert_eq!(manifest.files, done.files);
        assert_eq!(done.manifest.path.as_str(), "backups/vms/0a1b2c3d4e5f/20261112-030000/manifest.json");
        // A second backup into the same folder is refused, the first stays.
        assert!(back_up(&job, 1_793_000_001, &mut |_| {}).is_err());
        assert!(folder.join("disk0.raw.zst").exists());
    }

    #[test]
    fn a_disk_that_is_not_its_size_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        disk(&root, "disk0");
        let mut job = job(&root);
        job.disks[0].volsize = 8 << 20;
        let error = back_up(&job, 0, &mut |_| {}).unwrap_err().to_string();
        assert!(error.contains("not the 8388608"), "{error}");
        let folder = root.join("share/backups/vms/0a1b2c3d4e5f/20261112-030000");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    }

    #[test]
    fn a_restore_writes_the_disk_sparse_checks_the_hash_and_punches_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (_, data) = disk(&root, "disk0");
        let job = job(&root);
        let done = back_up(&job, 0, &mut |_| {}).unwrap();
        // The target: larger than the image, full of old data.
        let target = root.join("target");
        std::fs::write(&target, vec![7u8; 6 << 20]).unwrap();
        let nvram = root.join("libvirt/nvram/zendiloos-x_VARS.fd");
        let files = vec![
            RestoreFile { file: done.files[0].clone(), target: target.clone(), owner: None },
            RestoreFile { file: done.files[1].clone(), target: nvram.clone(), owner: None },
        ];
        let restore_job = VmRestoreJob { root: root.join("share"), folder: job.folder.clone(), files };
        let mut told = Vec::new();
        restore(&restore_job, &mut |percent| told.push(percent)).unwrap();
        let written = std::fs::read(&target).unwrap();
        assert_eq!(written.len(), 6 << 20, "the disk keeps its size");
        assert_eq!(&written[..4 << 20], &data[..]);
        assert!(written[4 << 20..].iter().all(|b| *b == 0), "what lies after the image is gone");
        assert_eq!(std::fs::read(&nvram).unwrap(), b"vars");
        assert_eq!(std::fs::metadata(&nvram).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(told.last(), Some(&100));

        // Another file of the same name and size is refused before the
        // disk is done: its hash differs.
        let image = root.join("share").join(job.folder.as_str()).join("disk0.raw.zst");
        let mut other = data.clone();
        other[10] = 1;
        let compressed = zstd::encode_all(&other[..], 3).unwrap();
        let mut file = VmBackupFile { bytes: compressed.len() as u64, ..done.files[0].clone() };
        std::fs::remove_file(&image).unwrap();
        std::fs::write(&image, &compressed).unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o644)).unwrap();
        let swapped = VmRestoreJob {
            files: vec![RestoreFile { file: file.clone(), target: target.clone(), owner: None }],
            ..restore_job.clone()
        };
        let error = restore(&swapped, &mut |_| {}).unwrap_err().to_string();
        assert!(error.contains("SHA-256 differs"), "{error}");
        // A file others may write is not restored at all.
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o666)).unwrap();
        file.sha256 = digest(&sha256(&compressed)).unwrap();
        let open = VmRestoreJob { files: vec![RestoreFile { file, target, owner: None }], ..restore_job };
        let error = restore(&open, &mut |_| {}).unwrap_err().to_string();
        assert!(error.contains("written by others"), "{error}");
    }

    #[test]
    fn removing_a_backup_takes_its_files_and_empty_folders_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        disk(&root, "disk0");
        let job = job(&root);
        back_up(&job, 0, &mut |_| {}).unwrap();
        let folder = root.join("share").join(job.folder.as_str());
        std::fs::write(folder.join("notes.txt"), "mine").unwrap();
        let remove_job = VmRemoveJob {
            root: root.join("share"),
            folder: job.folder.clone(),
            files: vec![
                VmBackupPart::Disk { slot: VmDiskSlot::new(0).unwrap(), volsize: 4 << 20 },
                VmBackupPart::Nvram,
            ],
        };
        assert!(remove(&remove_job).unwrap());
        assert!(folder.join("notes.txt").exists(), "a file of someone's stays, and so does its folder");
        assert!(!folder.join("disk0.raw.zst").exists() && !folder.join("manifest.json").exists());
        std::fs::remove_file(folder.join("notes.txt")).unwrap();
        assert!(!remove(&remove_job).unwrap(), "nothing more to remove");
        assert!(!folder.exists() && !folder.parent().unwrap().exists(), "the empty folders went");
        assert!(root.join("share/backups/vms").exists());
    }
}
