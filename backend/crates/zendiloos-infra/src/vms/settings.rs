//! A VM's settings as JSON: what a backup's manifest carries and its record
//! keeps, to restore the VM from. Read leniently where later versions may
//! add fields, strictly where a value must be one zendiloOS knows.

use serde::{Deserialize, Serialize};
use zendiloos_vms::domain::{
    BootDevice, Disk, DiskBus, Drive, Firmware, Location, Network, Nic, NicModel, OsPreset, UsbDevice,
    VmSettings,
};

/// The format of the settings: a newer one is not read.
const FORMAT: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Settings {
    format: u32,
    name: String,
    os: String,
    cpus: u16,
    memory_mib: u32,
    firmware: String,
    disks: Vec<DiskJson>,
    nics: Vec<NicJson>,
    drives: Vec<DriveJson>,
    boot: Vec<String>,
    autostart: bool,
    #[serde(default)]
    usb: Vec<String>,
    #[serde(default)]
    pci: Vec<String>,
    #[serde(default)]
    tpm: bool,
    #[serde(default)]
    secure_boot: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct DiskJson {
    slot: u8,
    size_gib: u32,
    bus: String,
    pool: String,
    volume: u16,
}

#[derive(Debug, Serialize, Deserialize)]
struct NicJson {
    slot: u8,
    model: String,
    network: String,
    #[serde(default)]
    device: Option<String>,
    mac: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DriveJson {
    slot: u8,
    image: Option<String>,
}

/// The settings as JSON.
pub fn to_json(settings: &VmSettings) -> String {
    let json = Settings {
        format: FORMAT,
        name: settings.name.clone(),
        os: settings.os.as_str().to_owned(),
        cpus: settings.cpus,
        memory_mib: settings.memory_mib,
        firmware: settings.firmware.as_str().to_owned(),
        disks: settings
            .disks
            .iter()
            .map(|disk| DiskJson {
                slot: disk.slot,
                size_gib: disk.size_gib,
                bus: disk.bus.as_str().to_owned(),
                pool: disk.location.pool.clone(),
                volume: disk.location.volume,
            })
            .collect(),
        nics: settings
            .nics
            .iter()
            .map(|nic| NicJson {
                slot: nic.slot,
                model: nic.model.as_str().to_owned(),
                network: nic.network.kind().to_owned(),
                device: nic.network.device().map(str::to_owned),
                mac: nic.mac.clone(),
            })
            .collect(),
        drives: settings
            .drives
            .iter()
            .map(|drive| DriveJson { slot: drive.slot, image: drive.image.clone() })
            .collect(),
        boot: settings.boot.iter().map(|device| device.to_text()).collect(),
        autostart: settings.autostart,
        usb: settings.usb.iter().map(|device| device.id()).collect(),
        pci: settings.pci.clone(),
        tpm: settings.tpm,
        secure_boot: settings.secure_boot,
    };
    serde_json::to_string(&json).unwrap_or_default()
}

/// The settings from JSON [`to_json`] wrote.
pub fn from_json(text: &str) -> Result<VmSettings, String> {
    let json: Settings =
        serde_json::from_str(text).map_err(|e| format!("the settings cannot be read: {e}"))?;
    if json.format != FORMAT {
        return Err(format!("settings of format {} are not known", json.format));
    }
    let bad = |what: &str, value: &str| format!("{what} {value} is not known");
    Ok(VmSettings {
        name: json.name,
        os: OsPreset::parse(&json.os).ok_or_else(|| bad("the system", &json.os))?,
        cpus: json.cpus,
        memory_mib: json.memory_mib,
        firmware: Firmware::parse(&json.firmware).ok_or_else(|| bad("the firmware", &json.firmware))?,
        disks: json
            .disks
            .into_iter()
            .map(|disk| {
                Ok(Disk {
                    slot: disk.slot,
                    size_gib: disk.size_gib,
                    bus: DiskBus::parse(&disk.bus).ok_or_else(|| bad("the bus", &disk.bus))?,
                    location: Location { pool: disk.pool, volume: disk.volume },
                })
            })
            .collect::<Result<_, String>>()?,
        nics: json
            .nics
            .into_iter()
            .map(|nic| {
                Ok(Nic {
                    slot: nic.slot,
                    model: NicModel::parse(&nic.model).ok_or_else(|| bad("the card", &nic.model))?,
                    network: Network::of(&nic.network, nic.device)
                        .ok_or_else(|| bad("the network", &nic.network))?,
                    mac: nic.mac,
                })
            })
            .collect::<Result<_, String>>()?,
        drives: json.drives.into_iter().map(|drive| Drive { slot: drive.slot, image: drive.image }).collect(),
        boot: json
            .boot
            .iter()
            .map(|device| BootDevice::parse(device).ok_or_else(|| bad("the boot device", device)))
            .collect::<Result<_, String>>()?,
        autostart: json.autostart,
        usb: json
            .usb
            .iter()
            .map(|device| UsbDevice::parse(device).ok_or_else(|| bad("the USB device", device)))
            .collect::<Result<_, String>>()?,
        pci: json.pci,
        tpm: json.tpm,
        secure_boot: json.secure_boot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_go_to_json_and_back_whole() {
        let mut settings = VmSettings::preset("Büro", OsPreset::Windows);
        settings.disks[0].location = Location { pool: "tank".into(), volume: 1 };
        settings.nics[0].mac = "52:54:00:00:00:01".into();
        settings.nics.push(Nic {
            slot: 1,
            model: NicModel::Virtio,
            network: Network::Bridge("br0".into()),
            mac: "52:54:00:00:00:02".into(),
        });
        settings.drives[0].image = Some("00000000000a".into());
        settings.usb = vec![UsbDevice { vendor: 0x046d, product: 0xc31c }];
        settings.pci = vec!["0000:01:00.0".into()];
        let json = to_json(&settings);
        assert!(json.contains("\"format\":1") && json.contains("\"046d:c31c\""), "{json}");
        assert_eq!(from_json(&json).unwrap(), settings);
        // Older settings without the devices of later versions.
        let old = json.replace(
            ",\"usb\":[\"046d:c31c\"],\"pci\":[\"0000:01:00.0\"],\"tpm\":true,\"secure_boot\":true",
            "",
        );
        let read = from_json(&old).unwrap();
        assert!(read.usb.is_empty() && !read.tpm, "{old}");
        assert!(from_json(&json.replace("\"format\":1", "\"format\":2")).unwrap_err().contains("format 2"));
        assert!(from_json(&json.replace("\"windows\"", "\"macos\"")).is_err());
        assert!(from_json("[]").is_err());
    }
}
