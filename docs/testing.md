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
| Installation ISO | the installer and the first setup | a fresh QEMU machine installed from the ISO, unattended (`just iso-test`) |
| Real hardware | compatibility, performance, everyday use | not yet (see below) |

CI runs `just check` on every push and pull request, and builds the package
and the ISO. The VM tests are run by hand before changes are merged; they
are not part of CI yet.

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
| `vm-hostname-test` | renaming the machine: the host name, `/etc/hosts`, Samba's NetBIOS name, `<name>.local` through Avahi, the certificate; the name put back |
| `vm-web-server-test` | the web interface's ports and certificates, ACME |
| `vm-notify-test` | notifications by e-mail and webhook |
| `vm-network-test` | changing a card's addresses, a change that cuts the VM off going back |
| `vm-network-takeover-test` | taking the network over from ifupdown, confirmed and not (VM 3) |
| `vm-network-bridge-test` | bridges, bonds and VLANs, frames bridged past Docker's FORWARD chain (VM 3, with spare cards) |
| `vm-docker-install-test` | installing Docker as the App Center's plugin |
| `vm-docker-test` | Docker projects, consent for what reaches beyond the containers, the terminal, image updates |
| `vm-project-pins-test` | a project runs as it was confirmed |
| `vm-docker-volumes-test` | Docker's volumes backed up, kept and restored |
| `vm-app-center-test` | an app from the catalog becoming an administrator's own project |
| `vm-vms-test` | virtual machines: the plugin, VMs with several disks and cards, editing, the image library, the console, live load |
| `vm-backup-test` | backup tasks: the plugin, mirrors and versions between pools, a USB drive when plugged in, an rsync daemon and SSH with a pinned host key, a pull, restores, an encrypted backup with rclone |
| `vm-update-test` | updates of Debian and zendiloOS through the interface, signed releases, refusals, a newer kernel without its ZFS module never started |
| `vm-upgrade-test` | a machine set up with an older package keeps its accounts, pools, shares and files when a newer one is installed |
| `iso-test` | installing from the ISO, the first setup, a pool, the network managed by systemd-networkd |

## Not tested yet

- **Real hardware.** zendiloOS has run only in QEMU machines. Trial
  operation on a real NAS with data one can lose is still to come.
- **Windows and macOS as clients.** SMB has been tested with `smbclient`
  inside the test VM, and discovery (WS-Discovery, Bonjour) is set up, but
  no Windows or macOS machine has used a share yet.
- **The bay order of the built-in NAS models** (the UGREEN DXP4800 Plus
  first) has not been confirmed on the real machine.
- **Backups to Google Drive and OneDrive.** Their sign-in and rclone's
  backends are tested against fakes only; no backup has gone to a real
  account yet.
