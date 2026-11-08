//! Where each disk sits: what the firmware calls the machine (DMI), and
//! making a disk's activity light blink so it can be found in its bay.
//!
//! Blinking is reading: for a moment, 1 MiB at a time from places spread
//! over the disk, so the drive's own cache answers none of it, opened
//! read-only with `O_DIRECT`, so the page cache does not either. Nothing
//! is ever written. Only a whole disk the kernel lists is read, checked
//! here as for SMART: a validated kernel name, its node in `/dev` (not a
//! link) and its entry in `/sys/class/block`, not a partition.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rustix::fs::{Mode, OFlags};
use zendiloos_agent_proto::{BurstMillis, DiskName, Response};

use crate::operations::OperationError;
use crate::pools::HostPaths;

const BLOCK: usize = 1024 * 1024;
/// `O_DIRECT` wants the buffer aligned to the disk's logical block size;
/// 4 KiB covers 512-byte and 4 KiB sectors alike.
const ALIGN: usize = 4096;
/// Far longer than any DMI name.
const MAX_DMI_BYTES: u64 = 256;

#[derive(Debug)]
pub(crate) struct Enclosure {
    dev: PathBuf,
    sys_block: PathBuf,
    dmi: PathBuf,
}

impl Enclosure {
    pub(crate) fn new(paths: &HostPaths) -> Self {
        Self { dev: paths.dev.clone(), sys_block: paths.sys_block.clone(), dmi: paths.dmi.clone() }
    }

    /// DMI's `sys_vendor` and `product_name`, raw; empty where the machine
    /// has none (no DMI, as on many ARM boards).
    pub(crate) fn machine(&self) -> Response {
        let field = |name: &str| {
            let mut text = String::new();
            if let Ok(file) = fs::File::open(self.dmi.join(name)) {
                let _ = file.take(MAX_DMI_BYTES).read_to_string(&mut text);
            }
            text
        };
        Response::Machine { sys_vendor: field("sys_vendor"), product_name: field("product_name") }
    }

    /// Reads from `device` for `millis`; the read under way when the time
    /// is up still ends.
    pub(crate) async fn burst(
        &self,
        device: &DiskName,
        millis: BurstMillis,
    ) -> Result<Response, OperationError> {
        let blocks = self.blocks(device)?;
        let path = self.dev.join(device.as_str());
        let length = Duration::from_millis(u64::from(millis.get()));
        tokio::task::spawn_blocking(move || read_for(&path, blocks, length))
            .await
            .map_err(|error| OperationError::Failed(format!("the read was cut off: {error}")))??;
        Ok(Response::Done)
    }

    /// Whole mebibytes on the disk, if it is a whole disk the kernel has.
    fn blocks(&self, device: &DiskName) -> Result<u64, OperationError> {
        let name = device.as_str();
        let sys = self.sys_block.join(name);
        if !sys.join("dev").exists() || sys.join("partition").exists() {
            return Err(OperationError::Refused(format!("there is no disk {name}")));
        }
        let size = sys.join("size");
        let sectors: u64 = fs::read_to_string(&size)
            .map_err(|e| OperationError::Read(size.clone(), e))?
            .trim()
            .parse()
            .map_err(|_| OperationError::Read(size, std::io::Error::other("not a number")))?;
        match sectors.saturating_mul(512) / BLOCK as u64 {
            0 => Err(OperationError::Refused(format!("disk {name} holds no medium"))),
            blocks => Ok(blocks),
        }
    }
}

/// Reads blocks of 1 MiB at places spread over `blocks` until `length` is
/// over; how many bytes it read.
fn read_for(path: &Path, blocks: u64, length: Duration) -> Result<u64, OperationError> {
    let flags = OFlags::RDONLY | OFlags::DIRECT | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NOCTTY;
    let disk = rustix::fs::open(path, flags, Mode::empty())
        .map_err(|e| OperationError::Read(path.to_owned(), e.into()))?;
    let mut buffer = vec![0u8; BLOCK + ALIGN];
    let start = buffer.as_ptr().align_offset(ALIGN);
    let block = &mut buffer[start..start + BLOCK];
    // Each burst starts elsewhere.
    let mut place =
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(1, |t| t.as_nanos() as u64) | 1;
    let begun = Instant::now();
    let mut read = 0;
    while begun.elapsed() < length {
        // xorshift64: cheap, and spread well enough over the disk.
        place ^= place << 13;
        place ^= place >> 7;
        place ^= place << 17;
        let offset = (place % blocks) * BLOCK as u64;
        let count = rustix::io::pread(&disk, &mut *block, offset)
            .map_err(|e| OperationError::Read(path.to_owned(), e.into()))?;
        read += count as u64;
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk `sdx` of `mib` MiB: a file as its node, its entry in sysfs.
    fn host(mib: u64) -> (tempfile::TempDir, Enclosure) {
        let root = tempfile::tempdir().unwrap();
        let (dev, sys, dmi) =
            (root.path().join("dev"), root.path().join("class-block"), root.path().join("dmi"));
        for dir in [&dev, &sys.join("sdx"), &sys.join("sdx1"), &dmi] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(dev.join("sdx"), vec![7u8; usize::try_from(mib).unwrap() * BLOCK]).unwrap();
        fs::write(sys.join("sdx/dev"), "8:0\n").unwrap();
        fs::write(sys.join("sdx/size"), format!("{}\n", mib * 2048)).unwrap();
        fs::write(sys.join("sdx1/dev"), "8:1\n").unwrap();
        fs::write(sys.join("sdx1/partition"), "1\n").unwrap();
        fs::write(sys.join("sdx1/size"), "2048\n").unwrap();
        let paths = HostPaths { dev, sys_block: sys, dmi, ..HostPaths::default() };
        (root, Enclosure::new(&paths))
    }

    fn disk(name: &str) -> DiskName {
        DiskName::parse(name).unwrap()
    }

    #[tokio::test]
    async fn a_burst_reads_the_whole_disk_for_its_time() {
        let (_root, enclosure) = host(4);
        let started = Instant::now();
        let answer = enclosure.burst(&disk("sdx"), BurstMillis::new(150).unwrap()).await.unwrap();
        assert_eq!(answer, Response::Done);
        assert!(started.elapsed() >= Duration::from_millis(150));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn reads_whole_blocks_spread_over_the_disk_and_writes_nothing() {
        let (root, _) = host(3);
        let path = root.path().join("dev/sdx");
        let before = fs::read(&path).unwrap();
        let read = read_for(&path, 3, Duration::from_millis(50)).unwrap();
        assert!(read > 0 && read.is_multiple_of(BLOCK as u64), "{read}");
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn only_whole_disks_the_kernel_lists_are_read() {
        let (root, enclosure) = host(4);
        let millis = BurstMillis::new(10).unwrap();
        let refused = |answer: Result<Response, OperationError>| match answer {
            Err(OperationError::Refused(reason)) => reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(refused(enclosure.burst(&disk("sdy"), millis).await), "there is no disk sdy");
        // A partition's entry, even under a disk's name.
        fs::write(root.path().join("class-block/sdx/partition"), "1\n").unwrap();
        assert_eq!(refused(enclosure.burst(&disk("sdx"), millis).await), "there is no disk sdx");
        fs::remove_file(root.path().join("class-block/sdx/partition")).unwrap();
        // An empty card reader.
        fs::write(root.path().join("class-block/sdx/size"), "0\n").unwrap();
        assert_eq!(refused(enclosure.burst(&disk("sdx"), millis).await), "disk sdx holds no medium");
        fs::write(root.path().join("class-block/sdx/size"), "many\n").unwrap();
        assert!(matches!(enclosure.burst(&disk("sdx"), millis).await, Err(OperationError::Read(..))));
        // A link in the node's place is not followed.
        fs::write(root.path().join("class-block/sdx/size"), "8192\n").unwrap();
        fs::remove_file(root.path().join("dev/sdx")).unwrap();
        std::os::unix::fs::symlink("/etc/hostname", root.path().join("dev/sdx")).unwrap();
        assert!(matches!(enclosure.burst(&disk("sdx"), millis).await, Err(OperationError::Read(..))));
    }

    #[test]
    fn the_machine_s_names_come_raw_and_empty_without_dmi() {
        let (root, enclosure) = host(1);
        assert_eq!(
            enclosure.machine(),
            Response::Machine { sys_vendor: String::new(), product_name: String::new() }
        );
        fs::write(root.path().join("dmi/sys_vendor"), "UGREEN\n").unwrap();
        fs::write(root.path().join("dmi/product_name"), "DXP4800 Plus\n").unwrap();
        assert_eq!(
            enclosure.machine(),
            Response::Machine { sys_vendor: "UGREEN\n".into(), product_name: "DXP4800 Plus\n".into() }
        );
    }
}
