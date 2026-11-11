//! An ISO uploaded from a browser into the VMs' image library
//! (docs/architecture.md, 4.13): the work of `receive-vm-image`, which the
//! system agent starts in a transient unit that may write only the VMs'
//! folder and has no capability at all.
//!
//! zendiloosd holds only the writing end of a pipe, the unit's standard
//! input; it never holds anything of the VMs' storage. The unit reads the
//! pipe into a file without a name in the library's folder (`O_TMPFILE`),
//! counting: past the size announced it stops and refuses. Only once the
//! pipe closed after exactly that many bytes is the file written out and
//! given its name, `<image>.iso`, which may not be taken; until then
//! nothing of it shows in the folder, and an upload cut off, refused or
//! given up leaves nothing behind. Where the file system has no
//! `O_TMPFILE`, a hidden name takes its place, removed on failure.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags};
use rustix::io::Errno;
use serde::{Deserialize, Serialize};
use zendiloos_agent_proto::{VM_IMAGE_UPLOAD_MAX, VmImageId};

use crate::volume_archive::{UnitLine, decode};

/// What is read from the pipe at once.
const CHUNK: usize = 1024 * 1024;

/// What the system agent asks of the unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UploadJob {
    /// `/volume<N>/@vms/isos`, made if missing.
    pub folder: PathBuf,
    /// `<image>.iso`.
    pub name: String,
    /// What zendiloosd announced: exactly this many bytes, or nothing.
    pub bytes: u64,
}

#[derive(Debug)]
enum UploadError {
    Refused(String),
    Failed(String),
}

fn failed(what: impl std::fmt::Display, error: impl std::fmt::Display) -> UploadError {
    UploadError::Failed(format!("{what}: {error}"))
}

/// Runs the job on `input` (the pipe), printing [`UnitLine`]s to `out`;
/// whether the image is in the library.
pub fn run_upload_job(hex: &str, input: &mut dyn Read, out: &mut dyn Write) -> bool {
    let outcome = decode::<UploadJob>(hex).map_err(UploadError::Refused).and_then(|job| receive(&job, input));
    let line = match outcome {
        Ok(bytes) => UnitLine::Done(serde_json::json!({ "bytes": bytes })),
        Err(UploadError::Refused(message)) => UnitLine::Failed { message, refused: true },
        Err(UploadError::Failed(message)) => UnitLine::Failed { message, refused: false },
    };
    let done = matches!(line, UnitLine::Done(_));
    let _ = writeln!(out, "{}", serde_json::to_string(&line).unwrap_or_default());
    let _ = out.flush();
    done
}

/// Where the content goes before it has a name.
enum Unnamed {
    /// Without a name at all: gone with the last descriptor.
    Anonymous,
    /// Under a hidden name, where there is no `O_TMPFILE`.
    Hidden(String),
}

fn receive(job: &UploadJob, input: &mut dyn Read) -> Result<u64, UploadError> {
    if VmImageId::of_file_name(&job.name).is_none() {
        return Err(UploadError::Refused(format!("{:?} is no name of an image", job.name)));
    }
    if job.bytes == 0 || job.bytes > VM_IMAGE_UPLOAD_MAX {
        return Err(UploadError::Refused(format!(
            "an uploaded image has 1 byte to {} GiB",
            VM_IMAGE_UPLOAD_MAX >> 30
        )));
    }
    match std::fs::create_dir(&job.folder) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(failed(job.folder.display(), error)),
    }
    let directory = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let folder = rustix::fs::open(&job.folder, directory, Mode::empty())
        .map_err(|e| failed(job.folder.display(), e))?;
    let mode = Mode::from_raw_mode(0o644);
    let (file, unnamed) =
        match rustix::fs::openat(&folder, ".", OFlags::TMPFILE | OFlags::WRONLY | OFlags::CLOEXEC, mode) {
            Ok(file) => (file, Unnamed::Anonymous),
            Err(Errno::OPNOTSUPP | Errno::ISDIR | Errno::INVAL | Errno::NOENT) => {
                let hidden = format!(".{}.part", job.name);
                let _ = rustix::fs::unlinkat(&folder, hidden.as_str(), AtFlags::empty());
                let flags =
                    OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
                let file = rustix::fs::openat(&folder, hidden.as_str(), flags, mode)
                    .map_err(|e| failed(&hidden, e))?;
                (file, Unnamed::Hidden(hidden))
            }
            Err(error) => return Err(failed(job.folder.display(), error)),
        };
    let kept = fill(&file, input, job.bytes).and_then(|()| name(&folder, &file, &unnamed, &job.name));
    if kept.is_err()
        && let Unnamed::Hidden(hidden) = &unnamed
    {
        let _ = rustix::fs::unlinkat(&folder, hidden.as_str(), AtFlags::empty());
    }
    kept.map(|()| job.bytes)
}

/// Exactly `bytes` from `input` into `file`, and on disk.
fn fill(file: &OwnedFd, input: &mut dyn Read, bytes: u64) -> Result<(), UploadError> {
    let mut buffer = vec![0u8; CHUNK];
    let mut received = 0u64;
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(failed("reading the upload", error)),
        };
        received += read as u64;
        if received > bytes {
            return Err(UploadError::Refused(format!(
                "the upload has more than the {bytes} bytes announced"
            )));
        }
        let mut written = 0;
        while written < read {
            written += rustix::io::write(file, &buffer[written..read])
                .map_err(|e| failed("writing the image", e))?;
        }
    }
    if received != bytes {
        return Err(UploadError::Refused(format!("the upload ended after {received} of {bytes} bytes")));
    }
    rustix::fs::fsync(file).map_err(|e| failed("writing the image", e))
}

/// Gives the complete file its name, which must not be taken, and writes
/// the folder out.
fn name(folder: &OwnedFd, file: &OwnedFd, unnamed: &Unnamed, name: &str) -> Result<(), UploadError> {
    let target = OsStr::from_bytes(name.as_bytes());
    let named = match unnamed {
        // The documented way to name an O_TMPFILE without CAP_DAC_READ_SEARCH.
        Unnamed::Anonymous => {
            let path = format!("/proc/self/fd/{}", file.as_raw_fd());
            rustix::fs::linkat(rustix::fs::CWD, path.as_str(), folder, target, AtFlags::SYMLINK_FOLLOW)
        }
        Unnamed::Hidden(hidden) => {
            rustix::fs::renameat_with(folder, hidden.as_str(), folder, target, RenameFlags::NOREPLACE)
        }
    };
    match named {
        Ok(()) => {}
        Err(Errno::EXIST) => return Err(UploadError::Refused(format!("{name} is in the library already"))),
        Err(error) => return Err(failed(name, error)),
    }
    rustix::fs::fsync(folder).map_err(|e| failed("the library's folder", e))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::volume_archive::encode;

    const NAME: &str = "00000000000a.iso";

    fn job(root: &std::path::Path, bytes: u64) -> String {
        encode(&UploadJob { folder: root.join("@vms/isos"), name: NAME.into(), bytes })
    }

    fn said(out: &[u8]) -> UnitLine {
        serde_json::from_str(String::from_utf8_lossy(out).trim()).unwrap()
    }

    fn entries(root: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root.join("@vms/isos"))
            .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn exactly_what_was_announced_is_named_in_the_library() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("@vms")).unwrap();
        let content: Vec<u8> = (0..3_000_000u32).map(|n| (n % 251) as u8).collect();
        let mut out = Vec::new();
        assert!(run_upload_job(&job(dir.path(), content.len() as u64), &mut content.as_slice(), &mut out));
        assert!(matches!(said(&out), UnitLine::Done(value) if value["bytes"] == 3_000_000));
        assert_eq!(fs::read(dir.path().join("@vms/isos").join(NAME)).unwrap(), content);
        assert_eq!(entries(dir.path()), [NAME], "nothing else is left");

        // The name is never taken from an image there is.
        let mut out = Vec::new();
        assert!(!run_upload_job(&job(dir.path(), 3), &mut b"new".as_slice(), &mut out));
        assert!(
            matches!(said(&out), UnitLine::Failed { refused: true, message } if message.contains("already"))
        );
        assert_eq!(fs::read(dir.path().join("@vms/isos").join(NAME)).unwrap().len(), 3_000_000);
    }

    #[test]
    fn an_upload_cut_short_or_too_long_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("@vms")).unwrap();
        for (announced, sent, says) in
            [(10, &b"short"[..], "after 5 of 10"), (3, &b"longer"[..], "more than")]
        {
            let mut out = Vec::new();
            assert!(!run_upload_job(&job(dir.path(), announced), &mut { sent }, &mut out));
            assert!(
                matches!(said(&out), UnitLine::Failed { refused: true, ref message } if message.contains(says)),
                "{:?}",
                said(&out)
            );
            assert!(entries(dir.path()).is_empty(), "{:?}", entries(dir.path()));
        }
    }

    #[test]
    fn only_an_image_of_the_library_within_the_limit_is_received() {
        let dir = tempfile::tempdir().unwrap();
        let bad = |name: &str, bytes: u64| {
            let hex = encode(&UploadJob { folder: dir.path().join("isos"), name: name.into(), bytes });
            let mut out = Vec::new();
            assert!(!run_upload_job(&hex, &mut b"x".as_slice(), &mut out), "{name} {bytes}");
            assert!(matches!(said(&out), UnitLine::Failed { refused: true, .. }));
        };
        bad("../00000000000a.iso", 1);
        bad("debian.iso", 1);
        bad(NAME, 0);
        bad(NAME, VM_IMAGE_UPLOAD_MAX + 1);
        assert!(!dir.path().join("isos").exists(), "refused before anything was made");
        let mut out = Vec::new();
        assert!(!run_upload_job("zz", &mut b"x".as_slice(), &mut out));
    }
}
