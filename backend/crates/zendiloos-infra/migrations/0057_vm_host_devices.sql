-- The host's USB and PCI devices a VM is given (zendiloos-vms,
-- docs/architecture.md, 4.13): USB by vendor and product, PCI by address;
-- libvirt binds a PCI device to vfio-pci only while the VM runs.
CREATE TABLE vm_host_devices (
    vm      TEXT    NOT NULL REFERENCES vms (id) ON DELETE CASCADE,
    kind    TEXT    NOT NULL CHECK (kind IN ('usb', 'pci')),
    -- `046d:c31c` for USB, `0000:01:00.0` for PCI.
    device  TEXT    NOT NULL CHECK (length(device) BETWEEN 9 AND 12),
    PRIMARY KEY (vm, kind, device)
) STRICT;
