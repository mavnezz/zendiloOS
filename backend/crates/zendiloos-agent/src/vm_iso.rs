//! An ISO copied from a share for a virtual machine (docs/architecture.md,
//! 4.13): the work of `copy-vm-iso`, which the system agent starts in a
//! transient unit that may write only the VMs' folder.
//!
//! The file is opened beneath the share's root, never through a link, onto
//! another file system or anything but a regular file (a pipe put there
//! would never end). The copy is made as `cp --reflink=auto` makes one: the
//! blocks cloned (`FICLONE`; OpenZFS clones blocks within a pool), else
//! `copy_file_range`, which clones too where it can, else copied, under a
//! hidden name that takes the copy's place only once it is complete and on
//! disk.

use std::ffi::OsStr;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::SharePath;

use crate::volume_archive::{UnitLine, decode};

/// Beneath the share, never through a link, never onto another file system.
const BENEATH: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS)
    .union(ResolveFlags::NO_XDEV);
/// What one call of `copy_file_range` is asked for: progress is told between.
const CHUNK: usize = 64 * 1024 * 1024;

/// What the system agent asks of the unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IsoJob {
    /// The share's root, `/volume<N>/<share>`.
    pub share: PathBuf,
    pub path: SharePath,
    /// `/volume<N>/@vms/isos`, made if missing.
    pub folder: PathBuf,
    /// `<id>.iso`.
    pub name: String,
}

#[derive(Debug)]
enum CopyError {
    Refused(String),
    Failed(String),
}

fn failed(what: impl std::fmt::Display, error: impl std::fmt::Display) -> CopyError {
    CopyError::Failed(format!("{what}: {error}"))
}

/// Runs the job, printing [`UnitLine`]s to `out`; whether the copy is there.
pub fn run_iso_job(hex: &str, out: &mut dyn Write) -> bool {
    let say = |out: &mut dyn Write, line: &UnitLine| {
        let _ = writeln!(out, "{}", serde_json::to_string(line).unwrap_or_default());
        let _ = out.flush();
    };
    let outcome = decode::<IsoJob>(hex).map_err(CopyError::Refused).and_then(|job| {
        copy(&job, &mut |percent| {
            say(out, &UnitLine::Progress { step: "copying_iso".into(), percent });
        })
    });
    match outcome {
        Ok(bytes) => {
            say(out, &UnitLine::Done(serde_json::json!({ "bytes": bytes })));
            true
        }
        Err(CopyError::Refused(message)) => {
            say(out, &UnitLine::Failed { message, refused: true });
            false
        }
        Err(CopyError::Failed(message)) => {
            say(out, &UnitLine::Failed { message, refused: false });
            false
        }
    }
}

fn copy(job: &IsoJob, progress: &mut dyn FnMut(u8)) -> Result<u64, CopyError> {
    if job.name.is_empty() || job.name.contains('/') || !job.name.ends_with(".iso") {
        return Err(CopyError::Refused(format!("{:?} is no name for an ISO's copy", job.name)));
    }
    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let root =
        rustix::fs::open(&job.share, directory, Mode::empty()).map_err(|e| failed(job.share.display(), e))?;
    let relative = job.path.as_str();
    if relative.is_empty() {
        return Err(CopyError::Refused("no file is named".into()));
    }
    let opened = rustix::fs::openat2(
        &root,
        relative,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        BENEATH,
    );
    let source = match opened {
        Ok(source) => source,
        Err(Errno::NOENT) => return Err(CopyError::Refused(format!("{relative} is not in the share"))),
        Err(Errno::XDEV | Errno::LOOP) => {
            return Err(CopyError::Refused(format!(
                "{relative} is reached through a link or another file system"
            )));
        }
        Err(error) => return Err(failed(relative, error)),
    };
    let stat = rustix::fs::fstat(&source).map_err(|e| failed(relative, e))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(CopyError::Refused(format!("{relative} is no file")));
    }
    let size = u64::try_from(stat.st_size).unwrap_or(0);

    match std::fs::create_dir(&job.folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(failed(job.folder.display(), error)),
    }
    let folder = rustix::fs::open(&job.folder, directory, Mode::empty())
        .map_err(|e| failed(job.folder.display(), e))?;
    let hidden = format!(".{}.part", job.name);
    let _ = rustix::fs::unlinkat(&folder, hidden.as_str(), rustix::fs::AtFlags::empty());
    let target = rustix::fs::openat(
        &folder,
        hidden.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )
    .map_err(|e| failed(&hidden, e))?;
    let written = fill(&source, &target, size, progress).and_then(|()| {
        rustix::fs::fsync(&target).map_err(|e| failed(&hidden, e))?;
        rustix::fs::renameat(&folder, hidden.as_str(), &folder, OsStr::from_bytes(job.name.as_bytes()))
            .map_err(|e| failed(&job.name, e))?;
        rustix::fs::fsync(&folder).map_err(|e| failed(job.folder.display(), e))
    });
    if written.is_err() {
        let _ = rustix::fs::unlinkat(&folder, hidden.as_str(), rustix::fs::AtFlags::empty());
    }
    written.map(|()| size)
}

/// The copy's content: cloned whole if the file system can, else in
/// chunks, telling how far.
fn fill(
    source: &OwnedFd,
    target: &OwnedFd,
    size: u64,
    progress: &mut dyn FnMut(u8),
) -> Result<(), CopyError> {
    progress(0);
    if rustix::fs::ioctl_ficlone(target, source).is_ok() {
        progress(100);
        return Ok(());
    }
    let mut done = 0u64;
    let mut told = 0u8;
    let mut plain = false;
    let mut buffer = Vec::new();
    while done < size {
        let wanted = usize::try_from((size - done).min(CHUNK as u64)).unwrap_or(CHUNK);
        let copied = if plain {
            copy_plain(source, target, &mut buffer, wanted)?
        } else {
            match rustix::fs::copy_file_range(source, None, target, None, wanted) {
                Ok(copied) => copied,
                // Not between these file systems: read and write.
                Err(Errno::XDEV | Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {
                    plain = true;
                    continue;
                }
                Err(error) => return Err(failed("copying the ISO", error)),
            }
        };
        if copied == 0 {
            return Err(CopyError::Failed("the ISO became shorter while it was copied".into()));
        }
        done += copied as u64;
        let percent = u8::try_from(done.saturating_mul(100) / size.max(1)).unwrap_or(100).min(100);
        if percent > told {
            told = percent;
            progress(percent);
        }
    }
    Ok(())
}

fn copy_plain(
    source: &OwnedFd,
    target: &OwnedFd,
    buffer: &mut Vec<u8>,
    wanted: usize,
) -> Result<usize, CopyError> {
    buffer.resize(wanted.min(4 * 1024 * 1024), 0);
    let read = rustix::io::read(source, &mut buffer[..]).map_err(|e| failed("reading the ISO", e))?;
    let mut written = 0;
    while written < read {
        written +=
            rustix::io::write(target, &buffer[written..read]).map_err(|e| failed("writing the copy", e))?;
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::volume_archive::encode;

    fn job(root: &std::path::Path, path: &str) -> IsoJob {
        IsoJob {
            share: root.join("volume1/media"),
            path: SharePath::parse(path).unwrap(),
            folder: root.join("volume1/@vms/isos"),
            name: "0a1b2c3d4e5f.iso".into(),
        }
    }

    fn lines(out: &[u8]) -> Vec<UnitLine> {
        String::from_utf8_lossy(out).lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }

    #[test]
    fn an_iso_is_copied_beside_the_vms_and_told_as_it_goes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("volume1/media/isos")).unwrap();
        fs::create_dir_all(dir.path().join("volume1/@vms")).unwrap();
        let content: Vec<u8> = (0..300_000u32).flat_map(u32::to_le_bytes).collect();
        fs::write(dir.path().join("volume1/media/isos/debian.iso"), &content).unwrap();

        let mut out = Vec::new();
        assert!(run_iso_job(&encode(&job(dir.path(), "isos/debian.iso")), &mut out));
        let copy = dir.path().join("volume1/@vms/isos/0a1b2c3d4e5f.iso");
        assert_eq!(fs::read(&copy).unwrap(), content);
        let said = lines(&out);
        assert!(matches!(said.first(), Some(UnitLine::Progress { percent: 0, .. })), "{said:?}");
        assert!(matches!(said.last(), Some(UnitLine::Done(_))), "{said:?}");
        assert!(!dir.path().join("volume1/@vms/isos/.0a1b2c3d4e5f.iso.part").exists());

        // A second copy replaces the first.
        fs::write(dir.path().join("volume1/media/isos/debian.iso"), b"new").unwrap();
        assert!(run_iso_job(&encode(&job(dir.path(), "isos/debian.iso")), &mut Vec::new()));
        assert_eq!(fs::read(&copy).unwrap(), b"new");
    }

    #[test]
    fn links_folders_and_missing_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("volume1/media/isos")).unwrap();
        fs::write(dir.path().join("secret"), b"root's").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), dir.path().join("volume1/media/isos/link.iso"))
            .unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("volume1/media/out")).unwrap();
        for path in ["isos/link.iso", "out/secret", "isos", "isos/missing.iso"] {
            let mut out = Vec::new();
            assert!(!run_iso_job(&encode(&job(dir.path(), path)), &mut out), "{path}");
            assert!(
                matches!(lines(&out).last(), Some(UnitLine::Failed { refused: true, .. })),
                "{path}: {out:?}"
            );
        }
        assert!(!dir.path().join("volume1/@vms/isos/0a1b2c3d4e5f.iso").exists());
        let mut bad = job(dir.path(), "isos/link.iso");
        bad.name = "../escape.iso".into();
        assert!(!run_iso_job(&encode(&bad), &mut Vec::new()));
    }
}
