//! The host's USB and PCI devices a VM is given: USB by vendor and product,
//! which stays when a device is plugged in elsewhere or again; PCI by its
//! address, only where the IOMMU is on, never one the host needs, and
//! always with every other device of its IOMMU group, as they go to a VM
//! together.

use std::collections::BTreeSet;

use super::{Invalid, invalid};

/// USB devices a VM is given at most.
pub const USB_MAX: usize = 8;
/// PCI devices a VM is given at most.
pub const PCI_MAX: usize = 8;

/// A USB device by its vendor and product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UsbDevice {
    pub vendor: u16,
    pub product: u16,
}

impl UsbDevice {
    /// `046d:c31c`, as `lsusb` writes it.
    pub fn id(self) -> String {
        format!("{:04x}:{:04x}", self.vendor, self.product)
    }

    /// From `046d:c31c`.
    pub fn parse(text: &str) -> Option<Self> {
        let (vendor, product) = text.split_once(':')?;
        let hex = |part: &str| (part.len() == 4).then(|| u16::from_str_radix(part, 16).ok()).flatten();
        Some(Self { vendor: hex(vendor)?, product: hex(product)? }).filter(|device| device.vendor != 0)
    }
}

/// Whether `text` is a PCI function's address as sysfs names it,
/// `0000:01:00.0`, in lowercase.
pub fn is_pci_address(text: &str) -> bool {
    let hex = |part: &str, length: usize| {
        part.len() == length && part.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    let Some((domain, rest)) = text.split_once(':') else { return false };
    let Some((bus, rest)) = rest.split_once(':') else { return false };
    let Some((slot, function)) = rest.split_once('.') else { return false };
    hex(domain, 4)
        && hex(bus, 2)
        && hex(slot, 2)
        && u8::from_str_radix(slot, 16).is_ok_and(|slot| slot < 32)
        && function.len() == 1
        && (b'0'..=b'7').contains(&function.as_bytes()[0])
}

/// A USB device plugged into the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUsb {
    pub device: UsbDevice,
    /// Where: `1-2.3`.
    pub port: String,
    pub manufacturer: Option<String>,
    pub name: Option<String>,
}

/// Why the host keeps a PCI device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostNeeds {
    /// A disk on it is in use (a pool, the system).
    Disk(String),
    /// The console's graphics.
    Console,
    /// The host's network is on it; none: which one is not known.
    Network(Option<String>),
    /// A bridge of the PCI bus.
    Bridge,
}

/// A PCI function of the host's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPci {
    pub address: String,
    pub vendor: u16,
    pub device: u16,
    /// `0x020000`: an Ethernet card.
    pub class: u32,
    pub vendor_name: Option<String>,
    pub device_name: Option<String>,
    pub iommu_group: Option<u32>,
    pub driver: Option<String>,
    pub needed: Option<HostNeeds>,
}

impl HostPci {
    /// A PCI-to-PCI bridge: it stays with the host, and the other devices
    /// of its group may go all the same.
    pub fn is_bridge(&self) -> bool {
        self.class >> 8 == 0x0604
    }
}

/// The IOMMU, and what says how to switch it on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Iommu {
    pub on: bool,
    /// What the kernel's command line says of it.
    pub cmdline: Vec<String>,
    pub cpu_vendor: Option<String>,
}

/// The host's devices a VM could be given.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostDevices {
    pub iommu: Iommu,
    pub usb: Vec<HostUsb>,
    pub pci: Vec<HostPci>,
}

impl HostDevices {
    /// The devices of `address`'s IOMMU group, itself among them, by
    /// address; none for a device without a group.
    pub fn group_of(&self, address: &str) -> Vec<&HostPci> {
        let Some(group) =
            self.pci.iter().find(|device| device.address == address).and_then(|d| d.iommu_group)
        else {
            return Vec::new();
        };
        self.pci.iter().filter(|device| device.iommu_group == Some(group)).collect()
    }

    /// Checks the USB devices and PCI addresses a VM is to be given; those
    /// it had `before` may stay whatever the host has now (a USB device
    /// unplugged meanwhile). A new USB device is plugged in, a PCI device
    /// goes only with an IOMMU, is no device the host needs and has every
    /// other device of its group (bridges aside) with it.
    pub fn check(
        &self,
        usb: &[UsbDevice],
        pci: &[String],
        before_usb: &[UsbDevice],
        before_pci: &[String],
    ) -> Result<(), Invalid> {
        let unique_usb: BTreeSet<&UsbDevice> = usb.iter().collect();
        if usb.len() > USB_MAX || unique_usb.len() != usb.len() || usb.iter().any(|device| device.vendor == 0)
        {
            return Err(invalid(format!(
                "a virtual machine is given up to {USB_MAX} USB devices, each once"
            )));
        }
        for device in usb.iter().filter(|device| !before_usb.contains(device)) {
            if !self.usb.iter().any(|found| found.device == *device) {
                return Err(invalid(format!("the USB device {} is not plugged in", device.id())));
            }
        }
        let unique_pci: BTreeSet<&String> = pci.iter().collect();
        if pci.len() > PCI_MAX || unique_pci.len() != pci.len() || !pci.iter().all(|a| is_pci_address(a)) {
            return Err(invalid(format!(
                "a virtual machine is given up to {PCI_MAX} PCI devices, each once"
            )));
        }
        if pci.iter().all(|address| before_pci.contains(address)) {
            return Ok(());
        }
        if !self.iommu.on {
            return Err(invalid("the IOMMU is off: no PCI device can be given to a virtual machine"));
        }
        for address in pci {
            let Some(device) = self.pci.iter().find(|device| device.address == *address) else {
                return Err(invalid(format!("there is no PCI device {address}")));
            };
            if device.iommu_group.is_none() {
                return Err(invalid(format!("the PCI device {address} is in no IOMMU group")));
            }
            for other in self.group_of(address) {
                if other.needed.is_some() && !(other.is_bridge() && other.address != *address) {
                    return Err(invalid(format!(
                        "the PCI device {address} goes only with {}, which the host needs",
                        other.address
                    )));
                }
                if other.needed.is_none() && !pci.contains(&other.address) {
                    return Err(invalid(format!(
                        "the PCI device {address} goes only with every device of its IOMMU group: {} too",
                        other.address
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pci(address: &str, class: u32, group: u32, needed: Option<HostNeeds>) -> HostPci {
        HostPci {
            address: address.into(),
            vendor: 0x10de,
            device: 1,
            class,
            vendor_name: None,
            device_name: None,
            iommu_group: Some(group),
            driver: None,
            needed,
        }
    }

    fn host() -> HostDevices {
        HostDevices {
            iommu: Iommu { on: true, ..Iommu::default() },
            usb: vec![HostUsb {
                device: UsbDevice { vendor: 0x046d, product: 0xc31c },
                port: "1-2".into(),
                manufacturer: None,
                name: None,
            }],
            pci: vec![
                pci("0000:00:01.0", 0x060400, 1, Some(HostNeeds::Bridge)),
                pci("0000:01:00.0", 0x030000, 1, None),
                pci("0000:01:00.1", 0x040300, 1, None),
                pci("0000:00:17.0", 0x010601, 2, Some(HostNeeds::Disk("sda".into()))),
                pci("0000:00:1f.0", 0x060100, 3, Some(HostNeeds::Bridge)),
                pci("0000:00:1f.3", 0x0c0500, 3, None),
            ],
        }
    }

    #[test]
    fn usb_devices_are_named_by_vendor_and_product_and_plugged_in_when_new() {
        let keyboard = UsbDevice { vendor: 0x046d, product: 0xc31c };
        assert_eq!(UsbDevice::parse("046d:c31c"), Some(keyboard));
        assert_eq!(keyboard.id(), "046d:c31c");
        for bad in ["046d", "046dc31c", "46d:c31c", "0000:c31c", "046d:c31g"] {
            assert_eq!(UsbDevice::parse(bad), None, "{bad}");
        }
        let stick = UsbDevice { vendor: 0x0781, product: 0x5583 };
        assert!(host().check(&[keyboard], &[], &[], &[]).is_ok());
        let error = host().check(&[stick], &[], &[], &[]).unwrap_err();
        assert!(error.0.contains("0781:5583 is not plugged in"), "{error}");
        assert!(host().check(&[stick], &[], &[stick], &[]).is_ok(), "one it had may be unplugged now");
        assert!(host().check(&[keyboard, keyboard], &[], &[], &[]).is_err());
        assert!(host().check(&[keyboard; 9], &[], &[keyboard], &[]).is_err());
    }

    #[test]
    fn pci_devices_go_with_their_whole_group_never_one_the_host_needs() {
        let gpu = || vec!["0000:01:00.0".to_owned(), "0000:01:00.1".to_owned()];
        assert!(host().check(&[], &gpu(), &[], &[]).is_ok(), "the root port stays, the rest goes");
        let half = host().check(&[], &gpu()[..1], &[], &[]).unwrap_err();
        assert!(half.0.contains("every device of its IOMMU group: 0000:01:00.1 too"), "{half}");
        let disk = host().check(&[], &["0000:00:17.0".to_owned()], &[], &[]).unwrap_err();
        assert!(disk.0.contains("which the host needs"), "{disk}");
        let lpc = host().check(&[], &["0000:00:1f.3".to_owned()], &[], &[]).unwrap_err();
        assert!(lpc.0.contains("0000:00:1f.0, which the host needs"), "{lpc}");
        let port = host().check(&[], &["0000:00:01.0".to_owned()], &[], &[]).unwrap_err();
        assert!(port.0.contains("which the host needs"), "{port}");
        let off = HostDevices { iommu: Iommu::default(), ..host() };
        assert!(off.check(&[], &gpu(), &[], &[]).unwrap_err().0.contains("IOMMU is off"));
        assert!(off.check(&[], &gpu(), &[], &gpu()).is_ok(), "kept as it was");
        assert!(
            host()
                .check(&[], &["0000:09:00.0".to_owned()], &[], &[])
                .unwrap_err()
                .0
                .contains("no PCI device")
        );
        assert!(host().check(&[], &["0000:01:00".to_owned()], &[], &[]).is_err());
        assert_eq!(host().group_of("0000:01:00.1").len(), 3);
        assert!(host().group_of("0000:09:00.0").is_empty());
        assert!(
            is_pci_address("0000:00:1f.7")
                && !is_pci_address("0000:00:20.0")
                && !is_pci_address("0000:00:1F.0")
        );
    }
}
