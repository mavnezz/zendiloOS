-- Windows 11 in a virtual machine (zendiloos-vms, docs/architecture.md,
-- 4.13): a software TPM 2.0 (swtpm), its state kept by libvirt in
-- /var/lib/libvirt/swtpm/<uuid>, and UEFI with Secure Boot and Microsoft's
-- keys enrolled. Both off for the VMs there are.
ALTER TABLE vms ADD COLUMN tpm INTEGER NOT NULL DEFAULT 0 CHECK (tpm IN (0, 1));
ALTER TABLE vms ADD COLUMN secure_boot INTEGER NOT NULL DEFAULT 0
    CHECK (secure_boot IN (0, 1) AND (secure_boot = 0 OR firmware = 'uefi'));
