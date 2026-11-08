# Zendiloos MVP and Roadmap

> As of 2026-11-08, version 0.2.0. Architecture and terms: [architecture.md](architecture.md).

## 1. MVP

The MVP was a NAS someone could use for real data on a home network,
delivered as a `.deb` package for Debian 13 amd64: setup and sign-in with
roles and an audit log, the desktop in the browser, system information,
metrics and logs, ZFS pools with SMART and scrubs, accounts and groups for
the web and SMB, SMB and NFS shares with a recycle bin, a file manager, and
background jobs with notifications. It shipped as 0.1 (0.1.0 on
2026-11-05, 0.1.1 on 2026-11-06, the first tagged release).

What it deliberately left out came with 0.2.0 (2026-11-08): Docker and
apps, network configuration, snapshots in the interface, notifications by
e-mail and webhook, two-factor sign-in, the installation ISO and updates
through the interface. Still later: AD/LDAP and arm64. Not planned at all:
FTP and WebDAV.

## 2. Roadmap

Every milestone ends with something that runs and a verifiable criterion.

| # | Milestone | Contents | Done when … |
|---|---|---|---|
| **M0** | Foundation (phase 1) | repository, workspace, configuration, logging, SQLite + migrations, authentication (setup, login, sessions, CSRF), agent skeleton + protocol + `zendiloos-sim`, OpenAPI → TypeScript types, Vue skeleton with login, `.deb` packages, Debian 13 test VM, CI | the packages are installed in the VM and one can log in through the browser |
| **M1** | Desktop (phase 2) | window manager, taskbar, launcher, notification center, dashboard with WebSocket metrics, settings (account, appearance, about), power | the desktop feels good in daily use, on phones too |
| **M2** | See, don't change (phase 3a) | disks, SMART, display of existing mdadm arrays/Btrfs volumes; users and groups (unified accounts); logs | the VM shows its virtual disks correctly and SMB users can be created |
| **M3** | Storage changes (phase 3b) | create and destroy ZFS pools with guard rails (plan, token, agent checks, device allowlist); job system; pool monitoring and disk replacement; scrub | create, degrade, replace and resilver are tested automatically in the VM |
| **M4** | Shares and files (phase 3c) | SMB, NFS, ACLs, recycle bin, file manager with "open as user" | a Windows/macOS client and the file manager work with the same permissions |
| **M5** | **MVP 0.1** | hardening, security review, documentation, upgrade test from M4 states | trial operation on real hardware with test data |
| **M6** | Advanced management (phase 4) | Docker through agent policy, app catalog (Compose), network (networkd), snapshots, notification channels | released in 0.2.0 |
| **M7** | Appliance (phase 5) | ISO with installer, A/B image updates, recovery, configuration backup, hardware support (LEDs, fans), arm64, SBOM/licensing | installation from ISO on the target NAS, update and rollback work |

Background jobs and notifications already arrive in M3 rather than
phase 4, because creating pools and resilvering are not usable without
them.

### Carried Over from storix

storix was an earlier attempt at the same product on NixOS (React
frontend, one core crate, a root helper `storix-privd`). It got further in
features but is weaker at the foundations: path checks as strings instead
of `openat2`, file access as the service user, separate web and NAS
accounts, NFS exports mapped to root, a Docker helper that accepts any
compose file. What carries over is domain logic with its tests and
interface ideas, adapted to the decisions in
[architecture.md](architecture.md); no module is taken over as a whole.
Compared on 2026-11-05.

| # | What | From storix | Changes for zendiloOS |
|---|---|---|---|
| M5 ✓ | storage health: one verdict, concerns ordered by severity | `storage/domain/overview.rs`, `Pool::is_redundant` | on the monitor's last state, not a SMART run per request; fill thresholds as in the monitor (80/90 %) |
| M5 ✓ | network display: interfaces and addresses | `infra/src/network` with `fixtures/ip-address.json` | read-only |
| M5 ✓ | stricter names for new files, path-escape test cases | `files/domain/path.rs`, API tests | new names only, so odd names written over SMB/NFS stay reachable |
| M5 ✓ | stop a scrub; rate and time left from the real `zpool status -j` fields; `recycle:exclude` for temporary and lock files; six minutes of metrics history on the server; more on the About page | various | storix's scan parser reads fields that real output lacks; computed from `issued` and `pass_start` instead |
| M6 ✓ | snapshots on demand and on a schedule, with retention, looked into and copied back from in the file manager, a share rolled back | `storage/domain/snapshot.rs`, `TakeDueSnapshots` | recognised by a user property, not a name prefix; the agent checks ownership; readable labels for Samba's `vfs_shadow_copy2` ("Previous Versions"); failures become notifications; rollback per share |
| M6 ✓ | Docker and the app catalog | `privd-proto` operations, `privd/src/docker.rs`, `infra/src/containers` with fixtures | apps from catalogs in the format of [zendiloOS.community.applications](https://github.com/mavnezz/zendiloOS.community.applications), the community catalog built in and others added; an install makes a Compose project of the administrator's own, confirmed like any project, with the catalog's own checks (`zendiloos-catalog`, pinned) on the normalized Compose file; Docker's data on a pool; installs as jobs (architecture 4.9) |
| M6 ✓ | share settings: quota, compression, record size | `storage/domain/dataset.rs` | per share (child dataset); no nested datasets |
| M6 ✓ | SMART self-tests on demand and on a schedule | `storage/domain/selftest.rs` | short or long, progress from `smartctl -j` (ATA status and NVMe log), stop; whether a drive can test itself read beforehand (QEMU's NVMe cannot, and `smartctl -t` still exits with 0); one plan per kind, disks by serial number, a four-hour window from a local hour (the host's time zone), the long test weekly at most |
| M6 ✓ | file manager: Range downloads, previews, restore from and empty the recycle bin, folders as tar, favourites per account | `routes/files.rs`, `files/tar.rs`, `files/domain/preview.rs` | through the files agent and fd passing; previews only for pictures, sound, video and text, sandboxed; restore works on the existing `.recycle/<user>` bins |
| M6 ✓ | the explorer: a tree of places (personal folder, favourites, volumes with their folders, the recycle bins together), details view, one list of actions for the toolbar and the context menu, keys and drag and drop; folders on a volume shared on the network or used on the NAS only; the Shares app folded into it | `files/lib/actions.ts`, `FileExplorer.vue`, `GET /files/volumes`, `shares.shared` | the toolbar went again, the context menu holds the actions; removable drives (FAT, exFAT, NTFS, ext4) have a place of their own |
| M6 ✓ | network configuration | `privd/src/network.rs`, `settings/domain/network.rs` | files under `/etc/systemd/network` written by the agent; confirm from the new address or the old configuration returns (timer, and at the agent's start); systemd-networkd over D-Bus; not yet reconciled from the database |
| M6 ✓ | SSH that switches itself off, the clock and its time servers | settings | on every installation; the host name and the ideas for A/B updates (M7) not yet |

Not carried over: FTP, rsync and WebDAV as storix had them (daemons
switched on and off, nobody could sign in; WebDAV is not wanted at all),
storix's trash and tags (shared by everybody, beside Samba's per-user
bins), nested datasets, external USB disks as storix had them (the
helper's checks were too weak; zendiloOS's removable drives are its own),
and storix's file access model.

### Status

| # | State | Verified |
|---|---|---|
| M0 | done | packages installed in the VM, login through nginx, a real reboot through API and agent with matching request IDs in audit log and journal |
| M1 | done | desktop with live metrics over WebSocket on a workstation and in the VM |
| M2 | done | in the VM: four SATA and one NVMe disk with SMART, the virtio system disk recognized as such, an mdadm RAID 1 with a Btrfs volume and its usage; an account created in the interface signs in to SMB, a wrong password does not; groups appear as Linux groups; the journal and the audit log are readable in the interface |
| M3 | done | `just vm-storage-test` in the VM, through the API: a ZFS mirror is created from a plan, one disk is pulled out of the kernel, the monitor reports the degraded pool, the disk is replaced through the API, the resilver finishes and is reported, the data is intact, a scrub finds no errors, the pool is destroyed |
| M4 | done, in review | `just vm-sharing-test` in the VM, through the API and with real clients inside the VM: over SMB (smbclient) a writer writes, a reader reads and cannot write, others cannot connect, and the share's ACLs match; a read-only NFS export with root squashed; the file manager, signed in as the same users, sees files written over SMB and SMB sees its uploads, with the same owners, inherited ACLs and refusals, while `..` and symbolic links do not lead out of the share; copying, renaming and deleting into the recycle bin as jobs; removed access reaches existing files; each writer's deletions land in their own recycle bin; Windows (WS-Discovery through `wsdd2`) and macOS (Bonjour through Avahi) can find the NAS. Fixed after the review on 2026-11-05: windows keep their size through a narrow browser; notifications are kept to the 500 most recent and read and cleared per account in every browser; language and theme are kept with the account; the monthly scrub gets the cron daemon that Debian's cloud image lacks. Not yet tried with a Windows or macOS client |
| M5 | released as 0.1.0 and 0.1.1 | the items taken over from storix; the upgrade test from M4 (`just vm-upgrade-test`); a security review on 2026-11-05 found two medium and five low issues, all fixed: setfacl followed links planted in a share, Argon2 runs were not limited, a recursive permission change could die after two minutes, one user's copies could starve the files agent, scrubs reached pools zendiloOS did not create, every role saw all jobs and notifications, agetty followed a file zendiloosd controls. Open: trial operation on real hardware, a Windows or macOS client |
| M6 | released in 0.2.0 | SMART self-tests on demand and on a schedule (in the VM: Berlin time, the planned disk only); share settings and snapshots with Previous Versions, looked into and copied back from, a share rolled back (`just vm-sharing-test`); the file manager package (range downloads, previews, restore and empty the recycle bin, folders as tar, favourites); the explorer with the Shares app folded in and a search below the folder shown; removable drives mounted, used and ejected (`just vm-removable-test`); notification channels by e-mail and webhook through one mail server (`just vm-notify-test`); network configuration with confirmation (`just vm-network-test`); the App Center: catalogs, Docker's data on a pool, apps installed as Compose projects of the administrator's own (`just vm-app-center-test`), Docker itself as its plugin; the Docker app with projects, containers, a terminal and image updates, what runs is what was confirmed (`just vm-docker-test`, `just vm-project-pins-test`); updates of Debian and zendiloOS by channel from GitHub Releases, signed (architecture 4.10, `docs/releases.md`, `scripts/vm-update-test.sh`); the control panel holds storage, the resource monitor and the logs; two-factor sign-in with an authenticator app and recovery codes (architecture 4.5, `just vm-two-factor-test`); a firewall with nftables for the NAS and the ports Docker publishes, changed with confirmation (architecture 4.12, `just vm-firewall-test`). Since: where each disk sits, from a built-in catalog of NAS models (the UGREEN DXP4800 Plus first, its bay order to be confirmed) or a layout of one's own, disks placed by port or serial number, a disk made to blink, the purchase and warranty of every disk seen (architecture 5, `just vm-enclosure-test`, not yet run in a VM); Docker's volumes with who uses them, backed up into a share on a schedule, kept by number and restored from an archive checked against its record (architecture 4.9, `just vm-docker-volumes-test`, not yet run in a VM). Open: the ACME plugin and the web interface's ports and certificate (architecture 4.11) not yet tried in a VM |
| M7 | in progress | the installation ISO installs unattended and opens the setup to its own network (`just iso-test`); signed updates by channel. Open: A/B updates and rollback, recovery, configuration backup, LEDs and fans, arm64, SBOM |

## 3. Test Environments

| Level | Purpose | How |
|---|---|---|
| Unit tests | models, rules, parsers, protocol validation | parsers against real tool output as fixtures (lsblk, smartctl, mdstat, zpool) |
| API tests | endpoints, authentication, permissions | `zendiloosd` in-process with a temporary SQLite database and `zendiloos-sim` |
| Development on a workstation | interface and API without root | `zendiloosd` with the simulated agent: fake disks, arrays, users |
| Debian 13 VM | agent, real tools, destructive operations | QEMU/KVM with the Debian cloud image, cloud-init, 4–6 virtual SATA/NVMe disks, packages installed by script; agent with device allowlist. The VM tests run in a second VM (`ZENDILOOS_VM=2`), the first one stays for trying things by hand |
| Installation ISO | the installer and the first setup | a fresh QEMU machine installed from the ISO, unattended (`just iso-test`) |
| Real hardware | compatibility, performance | from M5 on, with test data only |
