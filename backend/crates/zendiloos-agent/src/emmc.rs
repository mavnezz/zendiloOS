//! What an eMMC device reports of its wear. eMMC has no SMART; from
//! version 5.0 on its EXT_CSD register holds two estimates of the share of
//! its life used and how far its reserved blocks are used, which the kernel
//! shows as files of the card in sysfs (`life_time`, `pre_eol_info`).
//!
//! Read rather than run, as `/proc/mdstat`: the files are the interface,
//! read at fixed places below the validated name of a whole disk the kernel
//! has, never more than a few bytes each. What they mean is zendiloosd's
//! business.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use zendiloos_agent_proto::{DiskName, Response};

use crate::operations::OperationError;
use crate::pools::HostPaths;

/// Far longer than any of the card's files.
const MAX_FIELD_BYTES: u64 = 256;

#[derive(Debug)]
pub(crate) struct Emmc {
    /// `/sys/class/block`.
    sys_block: PathBuf,
}

impl Emmc {
    pub(crate) fn new(paths: &HostPaths) -> Self {
        Self { sys_block: paths.sys_block.clone() }
    }

    /// The card's files of `device`, raw; empty where it has none (a
    /// memory card, an eMMC older than 5.0).
    pub(crate) fn health(&self, device: &DiskName) -> Result<Response, OperationError> {
        let name = device.as_str();
        let disk = self.sys_block.join(name);
        if !name.starts_with("mmcblk") || !disk.join("dev").exists() || disk.join("partition").exists() {
            return Err(OperationError::Refused(format!("there is no eMMC device {name}")));
        }
        let card = disk.join("device");
        let field = |file: &str| {
            let mut text = String::new();
            if let Ok(file) = fs::File::open(card.join(file)) {
                let _ = file.take(MAX_FIELD_BYTES).read_to_string(&mut text);
            }
            text
        };
        Ok(Response::Emmc {
            kind: field("type"),
            life_time: field("life_time"),
            pre_eol_info: field("pre_eol_info"),
            name: field("name"),
            serial: field("serial"),
            fwrev: field("fwrev"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An eMMC `mmcblk0` with its partition and its card, and a disk `sda`.
    fn host() -> (tempfile::TempDir, Emmc) {
        let root = tempfile::tempdir().unwrap();
        let sys = root.path().join("class-block");
        for dir in ["mmcblk0/device", "mmcblk0p1", "sda"] {
            fs::create_dir_all(sys.join(dir)).unwrap();
        }
        for (file, text) in [
            ("mmcblk0/dev", "179:0\n"),
            ("mmcblk0p1/dev", "179:1\n"),
            ("mmcblk0p1/partition", "1\n"),
            ("sda/dev", "8:0\n"),
            ("mmcblk0/device/type", "MMC\n"),
            ("mmcblk0/device/life_time", "0x01 0x02\n"),
            ("mmcblk0/device/pre_eol_info", "0x01\n"),
            ("mmcblk0/device/name", "AJTD4R\n"),
        ] {
            fs::write(sys.join(file), text).unwrap();
        }
        let paths = HostPaths { sys_block: sys, ..HostPaths::default() };
        (root, Emmc::new(&paths))
    }

    #[test]
    fn the_cards_files_come_raw_and_empty_where_there_are_none() {
        let (_root, emmc) = host();
        let answer = emmc.health(&DiskName::parse("mmcblk0").unwrap()).unwrap();
        assert_eq!(
            answer,
            Response::Emmc {
                kind: "MMC\n".into(),
                life_time: "0x01 0x02\n".into(),
                pre_eol_info: "0x01\n".into(),
                name: "AJTD4R\n".into(),
                serial: String::new(),
                fwrev: String::new(),
            }
        );
    }

    #[test]
    fn only_a_whole_emmc_device_the_kernel_has_is_read() {
        let (_root, emmc) = host();
        for name in ["sda", "mmcblk1"] {
            let refused = emmc.health(&DiskName::parse(name).unwrap());
            assert!(matches!(refused, Err(OperationError::Refused(_))), "{name}: {refused:?}");
        }
    }
}
