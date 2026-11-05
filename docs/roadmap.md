# Zendiloos MVP and Roadmap

> As of 2026-11-05. Architecture and terms: [architecture.md](architecture.md).

## 1. MVP Definition

**The MVP (version 0.1) is a NAS someone could use for real data on a home
network**, delivered as `.deb` packages for Debian 13 amd64.

Included:

| Area | Scope |
|---|---|
| Access | initial setup with setup code, login, sessions, roles admin/user/readonly, audit log |
| Interface | desktop with windows, taskbar, launcher, notifications, light/dark mode, English (default) and German; home screen on phones |
| System | dashboard with live metrics (CPU, memory, network, disk I/O, temperatures), system information, logs, reboot/power off |
| Storage | disks with SMART status; create pools (basic, RAID 1/5/6/10 via mdadm) with a Btrfs or ext4 volume; state, resync progress, capacity; replace a disk in a degraded RAID; scheduled scrub |
| Users | accounts and groups, valid for web and SMB |
| Shares | SMB shares with per-user/group permissions, recycle bin (Samba `vfs_recycle`); NFS exports for hosts/networks |
| Files | file manager: browse, upload/download, create, rename, move, copy, delete (to recycle bin) |
| Operations | background jobs with progress; notifications for SMART warnings, degraded RAID, nearly full volumes |

Not in the MVP, deliberately later: Docker and apps, network configuration
(display only), ZFS, snapshots in the interface, email/push notifications,
2FA, AD/LDAP, ISO and updates through the interface, arm64.

## 2. Roadmap

Every milestone ends with something that runs and a verifiable criterion.

| # | Milestone | Contents | Done when … |
|---|---|---|---|
| **M0** | Foundation (phase 1) | repository, workspace, configuration, logging, SQLite + migrations, authentication (setup, login, sessions, CSRF), agent skeleton + protocol + `zendiloos-sim`, OpenAPI → TypeScript types, Vue skeleton with login, `.deb` packages, Debian 13 test VM, CI | the packages are installed in the VM and one can log in through the browser |
| **M1** | Desktop (phase 2) | window manager, taskbar, launcher, notification center, dashboard with WebSocket metrics, settings (account, appearance, about), power | the desktop feels good in daily use, on phones too |
| **M2** | See, don't change (phase 3a) | disks, SMART, display of existing mdadm arrays/Btrfs volumes; users and groups (unified accounts); logs | the VM shows its virtual disks correctly and SMB users can be created |
| **M3** | Storage changes (phase 3b) | create and destroy pools/volumes with guard rails (plan, token, agent checks, device allowlist); job system; RAID monitoring and disk replacement; scrub | create, degrade, replace and resync are tested automatically in the VM |
| **M4** | Shares and files (phase 3c) | SMB, NFS, ACLs, recycle bin, file manager with "open as user" | a Windows/macOS client and the file manager work with the same permissions |
| **M5** | **MVP 0.1** | hardening, security review, documentation, upgrade test from M4 states | trial operation on real hardware with test data |
| **M6** | Advanced management (phase 4) | Docker through agent policy, app catalog (Compose), network (networkd), snapshots, notification channels, ZFS module | |
| **M7** | Appliance (phase 5) | ISO with installer, A/B image updates, recovery, configuration backup, hardware support (LEDs, fans), arm64, SBOM/licensing | installation from ISO on the target NAS, update and rollback work |

Background jobs and notifications already arrive in M3 rather than
phase 4, because creating pools and resyncing RAID are not usable without
them.

## 3. Test Environments

| Level | Purpose | How |
|---|---|---|
| Unit tests | models, rules, parsers, protocol validation | parsers against real tool output as fixtures (lsblk, smartctl, mdadm, Btrfs) |
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
