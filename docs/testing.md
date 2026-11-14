# Testing

How zendiloOS is tested, and what has not been tested yet. Architecture and
terms: [architecture.md](architecture.md); what is still to be done is in
the [issues](https://github.com/mavnezz/zendiloOS/issues).

## Levels

| Level | Purpose | How |
|---|---|---|
| Unit tests | models, rules, parsers, protocol validation | parsers against real tool output as fixtures (lsblk, smartctl, mdstat, zpool); `just check` |
| Agent tests | the privileged agent's operations | the real agent against fake programs in a temporary root; `just check` |
| API tests | endpoints, authentication, permissions | `zendiloosd` in-process with a temporary SQLite database and `zendiloos-sim`; `just check` |
| Frontend tests | components and pages | Vitest with the API answered in the test; `just check` |
| Architecture tests | module boundaries | `zendiloos-architecture`; `just check` |
| Development on a workstation | interface and API without root | `just dev` with the simulated agent: fake disks, arrays, users |
| Debian 13 test VMs | the real agent, real tools, destructive operations | QEMU/KVM with Debian's cloud image, 4 SATA and 1 NVMe disk; `just vm-*-test` below |
| Installation ISO | the installer and the first setup | a fresh QEMU machine installed from the ISO, unattended (`just iso-test`, for arm64 `just iso-test arm64`; on GitHub the workflow *ISO test*, by hand) |
| Real hardware | compatibility, performance, everyday use | not yet (see below) |

CI runs `just check` on every push and pull request, runs the backend's
tests once more on an arm64 machine, and builds the packages for amd64 and
arm64 and, for a release, the ISOs. The VM tests and the installation from
the ISO are run by hand before changes are merged; they are not part of CI
yet.

## The VM tests

Each runs end to end against a test VM, through the HTTPS API as the
interface uses it, and looks inside the VM with the real tools. They change
and destroy things: run them in the second VM (`ZENDILOOS_VM=2`), or, where
noted, in a throwaway third one (`ZENDILOOS_VM=3`), never in the first,
which is for trying things by hand. Each script's header says what it
checks and how to run it.

| Test | What it covers |
|---|---|
| `vm-storage-test` | pools: create a mirror from a plan, a disk pulled out, the degraded pool reported, the disk replaced, resilver, scrub, delete |
| `vm-pool-delete-test` | deleting pools: refused while shares, Docker's data or VMs' storage are on them |
| `vm-enclosure-test` | where each disk sits: bays bound by port or serial number, a disk made to blink, the inventory |
| `vm-removable-test` | removable drives (FAT, exFAT, NTFS, ext4) plugged in, used, ejected and pulled out |
| `vm-sharing-test` | shares with real SMB and NFS clients inside the VM, snapshots and previous versions, the recycle bin |
| `vm-permissions-test` | share-level access: Samba, the file manager and NFS hold each account to its rule; one ACL on disk |
| `vm-services-test` | the file services' settings reaching Samba and NFS |
| `vm-accounts-test` | accounts, groups, personal folders, the password policy |
| `vm-two-factor-test` | two-factor sign-in with an authenticator app and recovery codes |
| `vm-security-test` | TLS, failed sign-ins and blocked addresses, sessions |
| `vm-firewall-test` | the firewall with nftables, Docker's published ports, a change that goes back unless confirmed |
| `vm-ssh-test` | SSH as a service: port, rules, switching itself off |
| `vm-config-backup-test` | the configuration backup: downloaded, on a schedule into a share with retention, refusals (passphrase, noise, a newer schema), restored on a fresh third VM instead of its setup with its accounts (UIDs, SMB passwords), name, time zone and SSH (VM 2 and VM 3) |
| `vm-pool-import-test` | a configuration backup restored on its machine installed afresh (`vm.sh reinstall`: a new system disk and host id, the spare disks kept): the pool found by `zpool import`, offered as this machine's own by the restored host id, imported when asked, its share's file reachable over SMB again and mounted after a reboot; the network's settings set aside without drift; the firewall's defaults until the backup's rules are confirmed, back unconfirmed (VM 3) |
| `vm-hostname-test` | renaming the machine: the host name, `/etc/hosts`, Samba's NetBIOS name, `<name>.local` through Avahi, the certificate; the name put back |
| `vm-web-server-test` | the web interface's ports and certificates, ACME |
| `vm-notify-test` | notifications by e-mail and webhook |
| `vm-ups-test` | a UPS: Network UPS Tools installed from its page, NUT's simulated UPS followed, a power cut and its end told, its state passed on to other devices with their account, the VM powered off below the charge set and, following a second NUT server as another device's, after its minutes on battery; started again, a UPS gone silent told |
| `vm-network-test` | changing a card's addresses, a change that cuts the VM off going back |
| `vm-network-takeover-test` | taking the network over from ifupdown, confirmed and not (VM 3) |
| `vm-network-bridge-test` | bridges, bonds and VLANs, frames bridged past Docker's FORWARD chain (VM 3, with spare cards) |
| `vm-docker-install-test` | installing Docker as the App Center's plugin |
| `vm-docker-test` | Docker projects, consent for what reaches beyond the containers, the terminal, image updates |
| `vm-project-pins-test` | a project runs as it was confirmed |
| `vm-docker-volumes-test` | Docker's volumes backed up, kept and restored |
| `vm-app-center-test` | an app from the catalog becoming an administrator's own project |
| `vm-vms-test` | virtual machines: the plugin, VMs with several disks and cards, editing, the image library, the console, live load |
| `vm-vm-backup-test` | VMs' snapshots of all disks at once, rolled back and compared by checksum, backups into a share restored as a new VM and over the old one, a backup canceled; a TPM and Secure Boot with Microsoft's keys; the IOMMU reported, USB passthrough where the test VM has USB (PCI passthrough skipped: the test VM has no IOMMU) |
| `vm-backup-test` | backup tasks: the plugin, mirrors and versions between pools, a USB drive when plugged in, an rsync daemon and SSH with a pinned host key, a pull, restores, an encrypted backup with rclone |
| `vm-update-test` | updates of Debian and zendiloOS through the interface, signed releases, refusals, a newer kernel without its ZFS module never started |
| `vm-upgrade-test` | a machine set up with an older package keeps its accounts, pools, shares and files when a newer one is installed |
| `iso-test` | installing from the ISO, the rescue entry in its menus, the first setup, a pool, the network managed by systemd-networkd, a configuration backup restored |

## arm64

What is tested of the arm64 build, and how:

- **The backend's tests run natively on arm64** in CI (*Rust on arm64*, on
  GitHub's `ubuntu-24.04-arm`, warnings failing too): what can differ
  between the architectures (ioctl numbers and struct sizes through
  `rustix`, the `zendiloos-exec` sandbox, `stat` fields, the C the crates
  bring) runs there, not only compiled. A release waits for it. `just
  check`, with the coverage, the frontend and the other gates, runs on
  amd64 only: the same code.
- **The package** is cross-compiled for arm64 in CI as on a workstation
  (`just deb arm64`); its programs are `aarch64` ELF files, its
  dependencies told by `dpkg-shlibdeps` from arm64's C library.
- **The update runner** takes only the package of its own architecture:
  the agent's tests refuse an amd64 package on an arm64 machine and a
  package whose name and `Architecture` disagree; zendiloosd picks each
  architecture's package from a release that holds both.
- **The installation ISO** for arm64 (`just iso-test arm64`, or the
  workflow *ISO test* with `arm64`) installs into QEMU's `virt` machine with
  UEFI (AAVMF) and runs the same checks as amd64's. Without KVM, on an x86
  machine and on GitHub's arm64 runners alike, QEMU emulates the CPU
  (TCG), which is far slower. Measured once on an x86 workstation (six
  cores, four of them emulating; 2026-11-12): the installer took 76
  minutes from its start to GRUB (the CD found on virtio-scsi, the disk
  partitioned for UEFI, the base system, `linux-headers-arm64`, the
  upgrade, SSH); zendiloOS's package, DKMS building ZFS and the first start
  come after that and were not waited for. The whole run takes some hours,
  so it is not part of CI: run it by hand (the workflow gives it 5½ hours)
  before a release that changes the installer, the package's dependencies
  or the ISO.
- **Not offered on arm64 and refused there**: virtual machines (the plugin
  says so, the runner refuses to install QEMU for x86), and apps of a
  catalog whose images are not made for arm64.

Not tested on arm64 yet: the installation from the ISO to its end (the
package installed, ZFS built, the first start and the checks after it;
the workflow *ISO test* with `arm64` does it), the programs running on
arm64 (only the tests have), real boards (the firmware, U-Boot's UEFI
without boot entries, SATA controllers and network cards of the boards), the VM
tests (the test VMs are amd64's), Docker apps actually running on arm64,
and updating an arm64 NAS from one release to the next (the checks are the
same as amd64's and tested in the agent's tests; the whole path needs an
arm64 release first).

## Not tested yet

- **Real hardware.** zendiloOS has run only in QEMU machines. Trial
  operation on a real NAS with data one can lose is still to come; on
  arm64 no real board at all (see [arm64](#arm64)).
- **A real UPS.** The VM test follows NUT's simulated UPS (`dummy-ups`).
  NUT's drivers for USB and SNMP, the search for UPSs on USB, a UPS that
  cuts its output after the shutdown and another make of NAS following
  zendiloOS's UPS have not been tried under zendiloOS yet.
- **Windows and macOS as clients.** SMB has been tested with `smbclient`
  inside the test VM, and discovery (WS-Discovery, Bonjour) is set up, but
  no Windows or macOS machine has used a share yet.
- **The bay order of the built-in NAS models** (the UGREEN DXP4800 Plus
  first) has not been confirmed on the real machine.
- **Backups to Google Drive and OneDrive.** Their sign-in and rclone's
  backends are tested against fakes only; no backup has gone to a real
  account yet.
