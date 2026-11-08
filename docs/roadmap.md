# Zendiloos MVP and Roadmap

> As of 2026-11-05. Architecture and terms: [architecture.md](architecture.md).

## 1. MVP Definition

**The MVP (version 0.1) is a NAS someone could use for real data on a home
network**, delivered as `.deb` packages for Debian 13 amd64.

Included:

| Area | Scope |
|---|---|
| Access | initial setup with setup code (open to the own network when installed from the ISO), login, sessions, roles admin/user/readonly, audit log |
| Interface | desktop with windows, taskbar, launcher, notifications, light/dark mode, English (default) and German; home screen on phones |
| System | dashboard with live metrics (CPU, memory, network, disk I/O, temperatures), system information, logs, reboot/power off |
| Storage | disks with SMART status; create ZFS pools (single disk, mirror, RAIDZ1/2/3), each one volume; state, resilver progress, capacity; replace a disk in a degraded pool; scheduled scrub |
| Users | accounts and groups, valid for web and SMB |
| Shares | SMB shares with per-user/group permissions, recycle bin (Samba `vfs_recycle`); NFS exports for hosts/networks |
| Files | file manager: browse, upload/download, create, rename, move, copy, delete (to recycle bin) |
| Operations | background jobs with progress; notifications for SMART warnings, degraded RAID, nearly full volumes |

Not in the MVP, deliberately later: Docker and apps, network configuration
(the MVP shows interfaces and addresses only), snapshots in the interface,
email/push notifications, two-factor sign-in, AD/LDAP, ISO and updates
through the interface, arm64. Not planned at all: FTP and WebDAV.

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
| **M6** | Advanced management (phase 4) | Docker through agent policy, app catalog (Compose), network (networkd), snapshots, notification channels | |
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
| M6 | Docker and the app catalog | `privd-proto` operations, `privd/src/docker.rs`, `infra/src/containers` with fixtures | apps from catalogs in the format of [zendiloOS.community.applications](https://github.com/mavnezz/zendiloOS.community.applications), the community catalog built in and others added like HACS; the agent runs the catalog's own checks (`zendiloos-catalog`, pinned) on the normalized Compose file and starts what it checked; own Compose files for administrators with a wider, confirmed policy; Docker's data on a pool; installs as jobs (architecture 4.9) |
| M6 ✓ | share settings: quota, compression, record size | `storage/domain/dataset.rs` | per share (child dataset); no nested datasets |
| M6 ✓ | SMART self-tests on demand and on a schedule | `storage/domain/selftest.rs` | short or long, progress from `smartctl -j` (ATA status and NVMe log), stop; whether a drive can test itself read beforehand (QEMU's NVMe cannot, and `smartctl -t` still exits with 0); one plan per kind, disks by serial number, a four-hour window from a local hour (the host's time zone), the long test weekly at most |
| M6 ✓ | file manager: Range downloads, previews, restore from and empty the recycle bin, folders as tar, favourites per account | `routes/files.rs`, `files/tar.rs`, `files/domain/preview.rs` | through the files agent and fd passing; previews only for pictures, sound, video and text, sandboxed; restore works on the existing `.recycle/<user>` bins |
| M6 ✓ | the explorer: a tree of places (personal folder, favourites, volumes with their folders, the recycle bins together), details view, one list of actions for the toolbar and the context menu, keys and drag and drop; folders on a volume shared on the network or used on the NAS only; the Shares app folded into it | `files/lib/actions.ts`, `FileExplorer.vue`, `GET /files/volumes`, `shares.shared` | removable drives get a place of their own later |
| M6 ✓ | network configuration | `privd/src/network.rs`, `settings/domain/network.rs` | files under `/etc/systemd/network` written by the agent; confirm from the new address or the old configuration returns (timer, and at the agent's start); systemd-networkd over D-Bus; not yet reconciled from the database |
| M7 | SSH that switches itself off, clock and hostname, ideas for A/B updates | settings and generations | appliance only; on an existing Debian these belong to the owner |

Not carried over: FTP, rsync and WebDAV as storix had them (daemons
switched on and off, nobody could sign in; WebDAV is not wanted at all),
storix's trash and tags (shared by everybody, beside Samba's per-user
bins), nested datasets, external USB disks for now (the helper's checks
were too weak), and storix's file access model.

### Status

| # | State | Verified |
|---|---|---|
| M0 | done | packages installed in the VM, login through nginx, a real reboot through API and agent with matching request IDs in audit log and journal |
| M1 | done | desktop with live metrics over WebSocket on a workstation and in the VM |
| M2 | done | in the VM: four SATA and one NVMe disk with SMART, the virtio system disk recognized as such, an mdadm RAID 1 with a Btrfs volume and its usage; an account created in the interface signs in to SMB, a wrong password does not; groups appear as Linux groups; the journal and the audit log are readable in the interface |
| M3 | done | `just vm-storage-test` in the VM, through the API: a ZFS mirror is created from a plan, one disk is pulled out of the kernel, the monitor reports the degraded pool, the disk is replaced through the API, the resilver finishes and is reported, the data is intact, a scrub finds no errors, the pool is destroyed |
| M4 | done, in review | `just vm-sharing-test` in the VM, through the API and with real clients inside the VM: over SMB (smbclient) a writer writes, a reader reads and cannot write, others cannot connect, and the share's ACLs match; a read-only NFS export with root squashed; the file manager, signed in as the same users, sees files written over SMB and SMB sees its uploads, with the same owners, inherited ACLs and refusals, while `..` and symbolic links do not lead out of the share; copying, renaming and deleting into the recycle bin as jobs; removed access reaches existing files; each writer's deletions land in their own recycle bin; Windows (WS-Discovery through `wsdd2`) and macOS (Bonjour through Avahi) can find the NAS. Fixed after the review on 2026-11-05: windows keep their size through a narrow browser; notifications are kept to the 500 most recent and read and cleared per account in every browser; language and theme are kept with the account; the monthly scrub gets the cron daemon that Debian's cloud image lacks. Not yet tried with a Windows or macOS client |
| M5 | in progress | the items taken over from storix; the upgrade test from M4 (`just vm-upgrade-test`); a security review on 2026-11-05 found two medium and five low issues, all fixed: setfacl followed links planted in a share, Argon2 runs were not limited, a recursive permission change could die after two minutes, one user's copies could starve the files agent, scrubs reached pools zendiloOS did not create, every role saw all jobs and notifications, agetty followed a file zendiloosd controls. Open: trial operation on real hardware, a Windows or macOS client |
| M6 | in progress | SMART self-tests on demand and on a schedule (in the VM: Berlin time, the planned disk only); share settings and snapshots with Previous Versions (`just vm-sharing-test`); the file manager package (range downloads, previews, restore and empty the recycle bin, folders as tar unpacked with GNU tar, favourites); notification channels by email and webhook. snapshots looked into and copied back from in the file manager. a share rolled back to a snapshot; network configuration with confirmation (`just vm-network-test` in the second VM); apps: catalogs like HACS, Docker's data on a pool, apps installed, run, updated and removed under the catalog's policy (in the second VM); Docker itself installed from the App Store, apps given a folder in a share (pinned, `just vm-app-folders-test`); updates of Debian and zendiloOS by channel from GitHub Releases, signed (architecture 4.10, `docs/releases.md`); the control panel holds storage, the resource monitor and the logs as pages of its own, no longer apps beside it. the ACME client as a plugin of the App Center (acme.sh as its own account, Let's Encrypt and its staging server, DNS-01 through a provider's API or HTTP-01, renewal a month ahead with notifications) and the web interface's ports and certificate with confirmation (architecture 4.11), not yet tried in a VM; two-factor sign-in to the web interface with an authenticator app (TOTP) and recovery codes, per account, reset by administrators (architecture 4.5; unit and API tests, not yet tried in the VM); a firewall with nftables for the NAS and the ports Docker publishes, changed with confirmation (architecture 4.12, `just vm-firewall-test`). Open: own Compose files for administrators |

## 3. Test Environments

| Level | Purpose | How |
|---|---|---|
| Unit tests | models, rules, parsers, protocol validation | parsers against real tool output as fixtures (lsblk, smartctl, mdstat, zpool) |
| API tests | endpoints, authentication, permissions | `zendiloosd` in-process with a temporary SQLite database and `zendiloos-sim` |
| Development on a workstation | interface and API without root | `zendiloosd` with the simulated agent: fake disks, arrays, users |
| Debian 13 VM | agent, real tools, destructive operations | QEMU/KVM with the Debian cloud image, cloud-init, 4–6 virtual SATA/NVMe disks, packages installed by script; agent with device allowlist |
| Real hardware | compatibility, performance | from M5 on, with test data only |

## 4. First Implementation Steps (M0)

Each step can be verified on its own and is presented separately.

1. **Repository skeleton:** Git, README, LICENSE, `.gitignore`,
   `.editorconfig`, development environment, `rust-toolchain.toml`,
   `justfile` with `fmt`, `lint`, `test`.
   *Check:* `just check` passes.
2. **Backend workspace with a first slice:** context `zendiloos-system`
   (domain + application with a `SystemInfoProvider` port), adapter in
   `zendiloos-infra` (`sysinfo`, os-release), primary adapter
   `zendiloos-api`, composition root `zendiloosd`; TOML configuration,
   `tracing`, graceful shutdown, `GET /api/v1/health` and
   `GET /api/v1/system/info`, OpenAPI at `/api/v1/openapi.json`, RFC 9457
   error format.
   *Check:* tests pass, `curl` returns JSON, no Clippy warnings.
3. **Database:** SQLite adapter in `zendiloos-infra` (WAL, embedded
   migrations, backup before migrating); first migration for accounts,
   sessions, audit.
   *Check:* migration tests, including restart with an existing database.
4. **Identity:** context `zendiloos-identity` (accounts, roles,
   permissions, sessions, throttling, audit) with SQLite repositories in
   `zendiloos-infra`; setup code, initial setup, login/logout, CSRF.
   *Check:* domain and use case tests against fakes; API tests for all paths
   including failed attempts and CSRF.
5. **Frontend skeleton:** Vite + Vue 3 + TypeScript strict + Tailwind 4 +
   shadcn-vue + Pinia + vue-query + vue-i18n; generated API types; setup and
   login pages; empty desktop.
   *Check:* `npm run build`, `vue-tsc`, ESLint, Vitest; login in the browser
   against `zendiloosd` with the simulated agent.
6. **Agent skeleton:** `zendiloos-exec`, `zendiloos-agent-proto`,
   `zendiloos-agent` with socket activation and `SO_PEERCRED`,
   `zendiloos-sim` behind `AgentTransport`, first agent gateway in
   `zendiloos-infra`; operations: version handshake, reboot, power off,
   reachable through `POST /api/v1/system/power` with audit. Disks follow
   with SMART in M2.
   *Check:* protocol tests, agent tests against a socket in a test
   directory.
7. **Packages and VM:** `debian/` with debhelper, built in a `debian:trixie`
   container; hardened systemd units, `sysusers.d`, nginx site; script for
   the Debian 13 VM with virtual disks.
   *Check:* installation in the VM, login in the browser through nginx.
8. **CI:** fmt, Clippy, tests, `cargo deny`, frontend checks, OpenAPI diff,
   package build.
