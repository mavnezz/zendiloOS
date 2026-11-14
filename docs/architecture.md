# Zendiloos Architecture

> Status: draft of 2026-11-05.
> **[Assumption]** marks assumptions, **[Open]** questions still to be
> decided, **[Later]** features deliberately postponed.

Zendiloos is a NAS operating system based on **Debian 13 "trixie"**. It is
managed entirely through a web desktop (Vue 3) that talks to a Rust backend.

The name `zendiloos` is used everywhere: crates `zendiloos-*`, services
`zendiloosd` and `zendiloos-agent`, system user `zendiloos`, paths
`/var/lib/zendiloos`, packages `zendiloos*`.

---

## 1. Target Architecture

### 1.1 Runtime View

```text
Browser (Vue desktop)
   │  HTTPS :443 (HTTP :80 optional)
   │  REST  /api/v1/…      WebSocket  /api/v1/ws
   v
nginx  ─────────────── /  →  static frontend  /usr/share/zendiloos/webui
   │  HTTP over Unix socket  /run/zendiloos/api.sock
   v
zendiloosd          user "zendiloos", no capabilities, never root
   ├─ SQLite        /var/lib/zendiloos/zendiloos.db  (source of truth)
   ├─ reads         /proc, /sys  (metrics, hardware information)
   ├─ tasks         metrics sampler, SMART/RAID monitoring, jobs, notifications
   │
   │  typed requests over Unix socket  /run/zendiloos/agent.sock
   │  (root:zendiloos 0660, plus SO_PEERCRED check)
   v
zendiloos-agent     root, socket-activated, no network, closed set of operations
   │  execve of fixed paths │ atomic files at fixed locations │ systemd via D-Bus │ Docker socket
   v
zpool · zfs · sgdisk · wipefs · smartctl · useradd · smbpasswd · docker · nft · …
```

| Process | Runs as | Responsibility | Network |
|---|---|---|---|
| nginx | `www-data` | TLS, static files, rate limiting, proxying | yes (80/443) |
| `zendiloosd` | `zendiloos` | API, authentication, business logic, database, live data | listens on a Unix socket only; connects out for updates, app catalogs, mail and webhooks |
| `zendiloos-agent` | `root` | privileged, validated single operations | none (`PrivateNetwork`) |

### 1.2 Two Deployment Modes, One Set of Packages

1. **Packages on Debian 13** (phases 1–4): the `.deb` package is installed
   on an existing system, or with the installation ISO (`just iso`):
   Debian 13's netinst image with preseeded answers, the package installed
   at the end (6).
2. **Appliance image** (phase 5): the same packages, preinstalled in a
   system image with its own installer and update mechanism.

This leads to a ground rule that applies from day one:

> **The database is the source of truth. System configuration (Samba, NFS
> exports, mount units, network, user accounts) is generated from it and
> reconciled at startup.**

This makes image-based updates possible later (the root filesystem can be
replaced without losing configuration), detects drift, and keeps Zendiloos
from editing configuration files it does not own. Zendiloos writes only its
own files (for example `/etc/samba/zendiloos.conf`, pulled in with
`include`) instead of rewriting `smb.conf`; the package puts its own
`smb.conf` in place only of Debian's unchanged default.

### 1.3 Installed Layout

The Debian package `zendiloos` (built with `just deb`) installs:

| Path | Purpose |
|---|---|
| `/usr/sbin/zendiloosd` | management service, user `zendiloos` (created through `sysusers.d`) |
| `/usr/libexec/zendiloos/zendiloos-agent` | privileged agent, root, started on demand |
| `/usr/share/zendiloos/webui/` | built web interface, served by nginx |
| `/etc/zendiloos/zendiloosd.toml`, `agent.toml` | static configuration (conffiles, defaults commented out) |
| `/etc/zendiloos/tls/` | self-signed certificate, created at installation or first boot by `/usr/libexec/zendiloos/make-certificate` (`zendiloos-certificate.service`) |
| `/etc/nginx/conf.d/zendiloos.conf` | static files, security headers, rate limits, proxy to the API socket; includes the files below for the HTTP server, the ports, the certificate and the TLS version |
| `/etc/zendiloos/web/` | `http.conf` (plain HTTP: a redirect to HTTPS that answers HTTP challenges, or none) and `https.conf` (ports and certificate), as set on the control panel's *Web interface* page; issued certificates in `certificates/` (4.11). Defaults written at installation, then by the agent |
| `/etc/zendiloos/nginx/tls.conf` | the oldest TLS version (`ssl_protocols`), written at installation, then by the agent |
| `/usr/lib/systemd/journald.conf.d/zendiloos.conf` | the journal at most 512 MiB on the system disk, in files of up to 64 MiB (6, writes to the system disk); a file of the administrator's in `/etc/systemd/journald.conf.d/` comes after it |
| `/etc/nfs.conf.d/zendiloos.conf` | fixed ports for NFSv3's helpers (mountd, statd, lockd), so the firewall can name them |
| `/etc/zendiloos/firewall/zendiloos.nft` | the firewall's rules, written by the agent once the firewall is on; `zendiloos-firewall.service` loads them with `nft` before the network comes up (4.12) |
| `/etc/zendiloos/apps/`, `/etc/zendiloos/projects/` | what the agent checked and runs, root's alone: apps installed the earlier way and administrators' Compose projects (4.9) |
| `/etc/zendiloos/vms/`, `/etc/tmpfiles.d/zendiloos-vms.conf`, `/run/zendiloos-vms/` | virtual machines (4.13): each domain's XML as the agent defined it; the consoles' folder, made at every boot, with QEMU's VNC sockets; their disks and the image library on a pool (`/volume<N>/@vms`) |
| `/etc/zendiloos/id-high-water` | the highest UID and GID the agent ever handed out (4.2) |
| `/etc/nut/` | Network UPS Tools' own files, written whole by the agent once a UPS is set (4.16); what stood in one before is kept beside it as `<name>.before-zendiloos` |
| `/var/lib/zendiloos-backup/` | the backup plugin's SSH key pair (`id_ed25519`), root's alone (0700), made by a unit of its own; reaches a run's unit only as its credential (4.14) |
| `/var/lib/zendiloos-acme/` | acme.sh's accounts, certificates and files for HTTP challenges, owned by the system user `zendiloos-acme`; a DNS provider's credentials only while it runs, in `/run/zendiloos-acme/` (4.11) |
| `/run/zendiloos-removable/` | the places removable drives are mounted at, a folder each, named by the UUID of their file system |
| `zendiloos-nfs-flush.service` | releases a share's NFS exports in the host's network namespace (3.3) |
| `zendiloos-console.service` | has the login prompts print the console's text again (`agetty --reload`) once the agent wrote it to `/run/issue.d` |
| `zendiloos-docker-bridges.service` | after every start of Docker and every change of the network's devices: lets frames within the bridges zendiloOS made pass Docker's `FORWARD` chain, its one rule in `DOCKER-USER` (4.12) |
| `/etc/systemd/system-generators/systemd-gpt-auto-generator` | a link to `/dev/null`, made at installation unless something is there already (the marker `/etc/zendiloos/masked-generators` says it is zendiloOS's; removed on purge): the generator mounts partitions of discoverable GPT types that fstab does not name, which a NAS installed with Debian's installer does not need, and on an eMMC system disk it fails at every reload of systemd ("Cannot dissect image: Input/output error") |
| `/etc/systemd/network/05-zendiloos-*` | the interfaces' settings and the bridges, bonds and VLANs zendiloOS made, written by the agent; while a change waits, its record `zendiloos-change.json` (6) |
| `/run/zendiloos/api.sock` | `zendiloosd.socket`, `zendiloos:www-data 0660`: only nginx reaches the API |
| `/run/zendiloos/agent.sock` | `zendiloos-agent.socket`, `root:zendiloos 0660`: only zendiloosd reaches the system agent |
| `/run/zendiloos/files.sock` | `zendiloos-files.socket`, `root:zendiloos 0660`: the files agent, the same program with `--role files` |
| `/etc/samba/smb.conf` | installed from `/usr/share/zendiloos/smb.conf` only if it is Debian's unchanged default (kept as `smb.conf.debian`); it includes `/etc/samba/zendiloos.conf`, which the agent writes |
| `/var/lib/zendiloos/` | database, backups, downloaded releases (`updates/`), setup code or the marker of an open setup (`setup-open`) (`0700`); a configuration backup's database staged for the next start (`restore/zendiloos.db`, 4.15), the one it replaced kept in `backup/` as `zendiloos-<time>-before-restore.db` |
| `/run/issue.d/zendiloos.issue` | the console text, written by the agent from its own template (0600): the login prompt shows the address and, until setup is done, the setup code, or for an open setup that the first administrator is created in the browser. zendiloosd only names the code; nothing it controls is read by agetty |

`zendiloosd` uses `Type=notify` and is ordered before `getty.target`, so the
console prompt appears only once the setup code (or the open setup) is
written. All services
run with strict systemd sandboxing; see 4.6 for what each may do.

### 1.4 Alternatives Considered

**Reverse proxy: nginx** rather than exposing axum directly. nginx provides
proven TLS, rate limiting, efficient static file serving, and later
forwarding to container apps. `zendiloosd` therefore never listens on a
network port. Downside: one more component to configure, which the package
takes care of.

**Privileged operations: a dedicated agent.**

| Option | Pros | Cons |
|---|---|---|
| **Dedicated agent with a closed set of operations** (chosen) | one point of validation and audit; validation in the type system; independent of polkit rules | our own root code, which must stay small |
| System D-Bus services (udisks2, NetworkManager, logind) + polkit | proven root code maintained elsewhere | generic object model, policy scattered across polkit rules; Samba, users and Docker still unsolved |
| `sudo` with a command list | quick to build | argument validation in sudoers is barely manageable; shell-adjacent attack surface |
| API runs as root | simplest | any flaw in the web service is immediately root |

---

## 2. Module Structure

### 2.1 Backend: DDD and Hexagonal Architecture

The backend follows domain-driven design with a hexagonal (ports and
adapters) architecture. The goal is the dependency rule: everything points
inward, and the domain knows no frameworks, databases or operating system.
**Crate boundaries enforce it**, so a violation is a build error, not a
review comment; what the compiler cannot see (a dependency the table does
not allow, I/O in the core, a domain layer reaching out) the architecture
tests in `zendiloos-architecture` catch, as part of `cargo test`.

| Concept | In Zendiloos |
|---|---|
| Bounded context | one core crate each: `identity`, `system`, `storage`, `sharing`, `apps`, `containers`, `updates`, `network`, `certificates`, `firewall`, `vms`, `backup`, `recovery` |
| Domain layer | entities, value objects, aggregates, domain rules and errors; pure Rust, no I/O |
| Application layer | use cases (commands and queries) and the **ports** they need, as traits |
| Primary (driving) adapter | `zendiloos-api`: HTTP and WebSocket |
| Secondary (driven) adapters | `zendiloos-infra`: SQLite repositories, agent gateways, host probes (`sysinfo`, `/proc`, journal), HTTP and mail clients (GitHub, app catalogs, webhooks, SMTP) |
| Shared kernel | `zendiloos-kernel`: IDs, validated names, `Secret`, time, common port errors |
| Composition root | `zendiloosd`: reads the configuration and plugs adapters into ports |

| Crate | Role | May depend on (internal) |
|---|---|---|
| `zendiloos-kernel` | shared kernel: time, `Secret`, request context, the audit entry and its port, what runs in the background (`RunningWork`), common port errors | – |
| `zendiloos-identity` | accounts and groups with their Linux users and groups, roles and permissions, sessions, passwords, two-factor sign-in (TOTP, recovery codes), throttling, audit | kernel |
| `zendiloos-system` | system information, metrics, power, logs, the SSH server, time and time zone, the machine's name (6), the web interface's oldest TLS version | kernel |
| `zendiloos-storage` | disks, SMART and its self-tests, ZFS pools and volumes, removable drives, plans for erasing disks, monitoring; where each disk sits (the enclosure, a disk made to blink) and every disk seen; mdadm arrays read-only | kernel, jobs, notify |
| `zendiloos-sharing` | SMB and NFS shares, share permissions, and the file manager (share contents as the signed-in user) | kernel, jobs, notify |
| `zendiloos-apps` | app catalogs and their sources, their apps filled in as templates of administrators' projects, apps installed the earlier way (4.9) | kernel, jobs |
| `zendiloos-containers` | the Docker engine as administrators see it: containers, images, networks, volumes, Compose projects (administrators' own with their file in a share), use over time, newer images on a schedule, volumes backed up on a schedule and restored, moving Docker's data (4.9) | kernel, jobs, notify |
| `zendiloos-updates` | updates of the NAS: Debian's packages and zendiloOS's releases from GitHub by channel, checked daily, installed on request (4.10) | kernel, notify |
| `zendiloos-network` | interfaces, addresses, default routes, name servers; an interface's IP settings and name servers through systemd-networkd, and bridges, bonds and VLANs made, changed and removed, each change with confirmation (6); the network taken over from ifupdown in a job, with confirmation (6); its settings as confirmed kept in the database and compared with the files at the start (6) | kernel, jobs, notify |
| `zendiloos-certificates` | the ACME client plugin (acme.sh): accounts, challenges and certificates from Let's Encrypt, renewed daily; the web interface's ports and certificate, changed with confirmation (4.11) | kernel, jobs, notify |
| `zendiloos-firewall` | the firewall: one ordered list of rules (interface, service or ports, source, allow or deny) and what happens when none matches, enforced by nftables for the NAS and the ports Docker publishes, changed with confirmation; services nobody reaches, told (4.12) | kernel, notify |
| `zendiloos-vms` | virtual machines: the plugin (libvirt, QEMU and swtpm installed, their storage in locations on one or more pools, the hardware check), VMs from an OS preset with their disks (each on its location), network cards (through the NAS, on a host bridge, directly on a card, on an internal network), CD drives, a TPM, Secure Boot and the host's USB and PCI devices, made in a job and changed (hardware only while off), snapshots of their disks by hand and on schedules, backups into a share or onto a removable drive and restores, as jobs, the image library (ISOs copied from a share or uploaded), what running VMs use, their state from libvirt and their consoles (4.13) | kernel, jobs |
| `zendiloos-backup` | backup tasks: the plugin (rsync, the SSH client and rclone installed, the NAS's SSH key), tasks from a share to another share or a removable drive, to an rsync server (daemon or SSH with the host key pinned) or from one, to Google Drive or OneDrive (accounts signed in to with OAuth), encrypted with rclone's crypt if wished, mirrors and dated versions with retention, schedules and "when its drive is plugged in", runs as jobs with their log and transfer lists, restores (4.14) | kernel, jobs, notify |
| `zendiloos-recovery` | the configuration backup: the archive made of the database, the host's files and settings and the accounts' Linux users, encrypted with a passphrase; downloaded, or written on a schedule into a folder of a share or a removable drive with the newest kept; restored on this or a fresh installation, also instead of the first setup (4.15) | kernel, jobs, notify |
| `zendiloos-ups` | the uninterruptible power supply: where it is (USB, SNMP, another device's NUT server), when the NAS shuts down on battery, whether the UPS cuts its output afterwards and whether other devices may follow it through the NAS; Network UPS Tools installed from Debian; its state read and a power cut, its end, a low battery, a battery to replace and a UPS gone silent told (4.16) | kernel, jobs, notify |
| `zendiloos-infra` | all secondary adapters, SQLite migrations | kernel, jobs, notify, contexts, agent-proto, exec; in tests also sim |
| `zendiloos-api` | primary adapter: axum routes, DTOs, OpenAPI, auth middleware, WebSocket | kernel, jobs, notify, contexts |
| `zendiloosd` | composition root (binary) | everything except agent |
| `zendiloos-agent-proto` | wire protocol to the agent, `AgentTransport` trait, Unix client | – |
| `zendiloos-exec` | the only way to run programs | – |
| `zendiloos-agent` | privileged root service (binary): the system agent, and with `--role files` the files agent | agent-proto, exec, fileops, logging |
| `zendiloos-fileops` | file operations beneath a share's root (`openat2`), with the credentials of the calling thread | agent-proto |
| `zendiloos-sim` | simulated agent behind `AgentTransport`; a model of a small NAS (disks, ZFS pools, resilver and scrub over time); executes nothing, but keeps share contents as real files in a scratch directory | agent-proto, fileops |
| `zendiloos-jobs` | long-running operations: recorded, published to subscribers, marked as interrupted after a restart; the running ones are the kernel's `RunningWork` | kernel |
| `zendiloos-notify` | notifications for administrators: a code with parameters, stored (the 500 most recent) and published; how far each account has read and cleared them; the mail server and the channels that carry them by email and to webhooks, and where each kind goes (4.3) | kernel |
| `zendiloos-logging` | logging setup shared by `zendiloosd` and the agent | – |
| `zendiloos-architecture` | tests only: checks this table and the rules below against the workspace (`cargo metadata`, and the sources parsed with `syn`) | – |

Crates are created in the step that needs them; there are no empty
placeholder crates.

```text
                         zendiloosd  (composition root)
               ┌──────────────┼──────────────────┬──────────────┐
               v              v                  v              v
        zendiloos-api   zendiloos-infra    zendiloos-sim   (agent side)
        primary adapter secondary adapters       │
               │         │            │           │
               v         v            v           v
      bounded contexts (core)      zendiloos-agent-proto <── zendiloos-agent ──> zendiloos-exec
      identity · system · storage          │                  (root)
      sharing · apps · containers          │
      updates · network                    │
      certificates · firewall · vms        │
      backup · recovery                    │
               │                           │
               └─────────────┬─────────────┘
                             v
                      zendiloos-kernel
```

Rules:

- **Context crates contain no I/O**, and neither do the kernel, jobs and
  notify. Their only outside dependencies are `thiserror`, `async-trait`,
  `tokio` and `tracing`: no sqlx, axum, sysinfo or serde. From tokio they use
  locks that serialize changes (`tokio::sync`), tasks and timers, and
  `tokio::fs::File` as the handle of a file the agent opened, which the file
  manager's port hands through; contexts never open files themselves. Their
  code (tests aside) names nothing of `std::fs`, `std::process`, `std::env`,
  `std::os`, `std::net` (addresses aside), `tokio::fs`, `tokio::process`,
  `tokio::net` or `tokio::signal`.
- **A domain layer stands on its own:** it names neither its application
  layer nor tokio or `async-trait`; ports and use cases live in
  `application/`.
- **Contexts do not depend on each other.** If one context needs something
  from another (sharing needs to know volumes), it declares a port in its
  own terms, and `zendiloos-infra` or the composition root implements that
  port. This is the anti-corruption layer between contexts.
- **`zendiloos-api` calls use cases, never adapters.** It does not depend on
  `zendiloos-infra`.
- **The agent side knows nothing about contexts, the database or HTTP.**
  A bug in a parser, a repository or a handler therefore never runs as
  root. Infra's agent gateways translate between context ports and the agent
  protocol; behind `AgentTransport` sits either the Unix socket client or
  `zendiloos-sim`, so gateway code including its parsers runs in tests and
  on a workstation without root or real disks.

Inside a context crate:

```text
zendiloos-storage/src/
├── lib.rs
├── domain/            entities, value objects, rules (is this RAID layout valid?)
│   ├── disk.rs
│   └── pool.rs
└── application/
    ├── ports.rs       traits the context needs: BlockDeviceSource, ZfsSource, PoolAdmin, …
    ├── commands.rs    use cases that change state, through the domain: CreatePool, …
    └── queries/       use cases that read: ListDisks, GetSmartReport, …
```

Deliberate simplifications:

- **No generic command or query bus.** A use case is a struct with an
  `execute` method; that is explicit and type-safe.
- **Lightweight CQRS.** Commands go through the domain model; queries may
  read a port and return a read model directly.
- **Depth per context.** `storage`, `sharing` and `identity` have real rules
  and use every layer. `system` is mostly a pass-through and stays thin.
- **Async ports** are traits with `#[async_trait]`, held as `Arc<dyn Port>`.
  Rust does not yet allow `dyn` traits with native `async fn`.
- **No event bus.** A use case applies its own changes through its ports
  (a share's Samba configuration through `ShareHost`). A context whose
  changes another must follow declares an observer port (Identity's
  `GroupObserver` and `AccountObserver`), which `zendiloos-infra` bridges
  to the context that follows (Sharing); no broker. A context that must ask
  another before it acts declares a port in its own terms the same way
  (Network's `DeviceUsers`: what is on a bridge that would go, or on a
  card that would be taken into a bridge or bond, answered from the VMs'
  records).

**Alternatives considered:**

| Split | Pros | Cons |
|---|---|---|
| **Core crate per context + one infra crate** (chosen) | domain purity and context separation enforced by the compiler; root code isolated | more crates; infra grows with every context |
| One crate per context with domain, application and adapters as modules | a context in one place, fewer crates | domain purity only by convention, since the crate depends on sqlx and sysinfo |
| One core crate with contexts as modules, plus infra and api | fewest crates; domain purity enforced | separation between contexts only by convention (`pub(crate)`) |

### 2.2 Frontend

| Purpose | Choice | Rationale |
|---|---|---|
| Framework | Vue 3 (Composition API, `<script setup>`), TypeScript 5.9 strict, Vite | TypeScript 7 once vue-tsc, typescript-eslint and openapi-typescript support it |
| Styling/components | Tailwind CSS 4, shadcn-vue (on Reka UI) | components live as source in the repo, accessible, adaptable |
| Client state | Pinia | windows, session, notifications; language and theme as kept with the account, with a copy in the browser for the sign-in page |
| Server state | `@tanstack/vue-query` | caching, invalidation, loading and error states; avoids rebuilding this in every Pinia store |
| API types | `openapi-typescript` + `openapi-fetch`, generated from utoipa | one source of truth, no drift between Rust and TypeScript |
| Routing | Vue Router: `/setup`, `/login`, `/password` (a password change the account must make first), `/` (desktop) | apps are opened through the window store, not through URLs |
| i18n | vue-i18n; English first (the default and the source of all texts), German as a second language chosen in the settings | retrofitting internationalization is expensive |
| Icons | lucide-vue-next | matches shadcn-vue |
| Tests | Vitest with Vue Test Utils, later Playwright for end-to-end tests | |
| Formatting and linting | Prettier; ESLint with the Vue and TypeScript rules and `import-x/no-restricted-paths` for module boundaries | Prettier formats, ESLint checks code only |

```text
frontend/src/
├── main.ts
├── app/                  composition root: App.vue, router with its guards, navigation
├── desktop/              shell
│   ├── windowManager.ts  pure logic (open, focus, minimize, move, resize,
│   │                     clamp geometry), unit tested with Vitest
│   ├── windows.ts        Pinia store around windowManager, layout in localStorage
│   ├── apps.ts           app registry: id, icon, window size, permission,
│   │                     lazily loaded component or sections; the control panel's pages
│   ├── apps/             the explorer's and the control panel's windows, and the
│   │                     control panel's own pages (appearance, language, update)
│   ├── DesktopShell.vue · DesktopDock.vue · AppLauncher.vue · AppWindow.vue
│   └── NotificationCenter.vue · ToastStack.vue · UserMenu.vue
├── features/             one module per area, handing pages to the desktop
│   ├── accounts/  apps/  auth/  channels/  docker/  files/
│   ├── logs/  network/  security/  storage/  system/  updates/  vms/
│   │   ├── api.ts        query/mutation composables (vue-query)
│   │   ├── lib/          pure logic, unit tested (where a feature has any)
│   │   └── components/   components and pages
└── shared/
    ├── api/              fetch client (CSRF, error format), WebSocket client, generated schema
    ├── ui/               shadcn-vue components, own building blocks (meter, sparkline, confirm dialog)
    ├── jobs/  notices/  preferences/   Pinia stores the features share
    ├── lib/              formatting, helpers
    ├── styles/           global CSS
    └── i18n/             translations
```

- Features do not import from each other, only from `shared/`.
  `desktop/apps.ts` composes feature pages into apps; features know nothing
  about windows. Enforced with an ESLint rule (`import-x/no-restricted-paths`).
- The desktop's apps are the file explorer, the App Center, Docker,
  Virtual machines (once its plugin is set up, with a console window of
  its own that only that app opens), Backup (once its plugin is
  installed, 4.14) and the control panel. Storage (pools, disks), the resource
  monitor and the logs (system log, audit log) are pages of the control
  panel, in its groups *Storage* and *Monitoring*, each shown to the
  accounts with the permission it needs (`storage.view`, `system.view`,
  `logs.view`); features hand them over as pages, never with a frame of
  their own, so a window has one navigation. The dock's resource meters
  open the monitor's page.
- The account's **interface preferences** (`account_preferences`,
  `GET/PATCH /api/v1/preferences`) are language, scheme, background, widgets
  and the order of the icons, kept with the account so that every browser
  looks the same. A browser sends only what was chosen in it, and a field
  left out stays as it is, so a tab loaded before another one chose
  something takes nothing back; before the account's choices arrived (or
  when loading them failed) a choice applies in that browser only and is
  not saved. A background may be a single colour (`color`, with its
  `#rrggbb`, kept while an image is chosen); its brightness sets
  `data-wallpaper-tone`, so icon labels turn dark on a light colour in
  either scheme, and glass lets less of it through in the dark one.
- The preferences also hold the **reminder to star zendiloOS on GitHub**:
  a card where the toasts appear, for administrators only, from the
  account's 14th day, at most once per browser session and twice in all,
  the second time 60 days after "Later"; the link and "No thanks" end it.
  It waits for a calm moment: no job running, no dialog open, no unread
  warning or error and the storage overview fine. The decision is one pure
  function (`shared/lib/star-prompt.ts`); the server only keeps how often
  it was shown, when, and the answer, and never lets that go back: an
  answer that ends it (the link, "No thanks") stays, whatever "Later" or
  showing another tab sends, and in whichever order; otherwise the one
  shown more often, later, or answered wins. It is
  only a link: nothing is asked of GitHub, counted or sent.
- Pages adapt to the window size with **container queries**, not to the
  screen size.
- On phones there are no windows: the desktop becomes a home screen and apps
  open full screen.

---

## 3. Communication Paths

### 3.1 Browser ↔ nginx ↔ zendiloosd

- **REST** under `/api/v1/…`, JSON with `snake_case` field names.
  Timestamps are integers: Unix time in seconds (UTC); the frontend formats
  them in the viewer's locale. Errors use RFC 9457
  (`application/problem+json`) with a stable `code` for the frontend;
  internal details go to the log only.
- **A change is seen through:** a request that is not safe (POST, PUT,
  PATCH, DELETE) runs to its end in a task of its own even when its client
  hangs up (a closed tab; nginx gives up after a minute), so the host and
  the records never part halfway; reading requests go with their client.
- **Versioning:** the path carries the major version. The OpenAPI document
  is generated, checked in, and verified in CI against unintended changes.
  A breaking change means `/api/v2` alongside `/api/v1`.
- **Long-running operations** (creating a pool, RAID resync, large copies,
  image pulls) answer `202 Accepted` with a job ID. Progress arrives over the
  WebSocket; jobs are also stored in the database so a restart can mark them
  as interrupted.
- **One WebSocket per browser tab** (`/api/v1/ws`) for live data, with
  topic subscriptions (`metrics`, `jobs`, `notifications`, `vm_stats`) instead of one
  connection per data stream. A subscription to `metrics` gets the latest
  sample at once; `jobs` brings the whole job at every change:

  ```json
  → {"type":"subscribe","topic":"metrics"}
  ← {"type":"metrics","data":{…}}
  ← {"type":"job","data":{"id":12,"kind":"storage.create_pool","state":"running","step":"creating_pool","progress":0.42,…}}
  ← {"type":"notification","data":{…}}
  → {"type":"unsubscribe","topic":"metrics"}
  ← {"type":"error","message":"…"}
  ```

  A single sampler task produces metrics (every 2 s) and distributes them
  through `tokio::sync::broadcast`/`watch`; the load does not depend on the
  number of open browsers. `vm_stats`, for `vms.manage` only, is sampled
  every 3 s by one task that runs only while someone is subscribed (4.13). A terminal in a container uses a WebSocket of
  its own (`/api/v1/docker/containers/{id}/terminal`).
- In production `zendiloosd` listens on `/run/zendiloos/api.sock`, during
  development on `127.0.0.1:8780` (`just dev`; Vite proxies to it).

### 3.2 zendiloosd ↔ zendiloos-agent

| Property | Decision |
|---|---|
| Transport | Unix sockets created by systemd (`root:zendiloos 0660`), agents socket-activated: `/run/zendiloos/agent.sock` for the system agent (disks, ZFS, accounts, server configuration), `/run/zendiloos/files.sock` for the files agent (share contents); each refuses the other's requests |
| Authentication | file permissions + `SO_PEERCRED`: only the UID of `zendiloos` is served |
| Framing | one request per connection; JSON lines (NDJSON); size limits (requests 2 MiB, responses 16 MiB); before the answer of a long operation, any number of `progress` lines with the step it is at (`creating_pool`) and how far in percent, which become the job's step in zendiloosd and are named in the interface, and `output` lines a program it runs wrote (Docker Compose's "Container x Recreated"), for a person to follow |
| Operations | closed `enum Request` with typed arguments validated during deserialization; `deny_unknown_fields` |
| Responses | one answer, after any number of `progress` and `output` lines |
| Files | for downloads and uploads the files agent passes the opened file's descriptor via `SCM_RIGHTS`, attached to the response line (see 4.4) |
| Version | every request carries the protocol `version`; another one is refused. `hello` answers with the agent's protocol version and package version; API and agent ship from the same package |
| Traceability | every operation carries a request ID that appears in the API's audit log and in the agent's journal; one that only reads the NAS's state (asked for many times a minute) only at debug, unless it fails |
| Limits | timeout per operation, bounded concurrency, destructive storage operations serialized |

Alternatives: D-Bus (zbus) would bring introspection and polkit, but costs
considerably more complexity for a single client. Varlink (increasingly used
by systemd) is interesting, but still rare in Rust. gRPC is oversized for a
local channel. A JSON protocol modelled with serde enums is fully typed and
simple to audit.

### 3.3 Agent ↔ Linux

- **Programs** only through `zendiloos-exec`: absolute paths from a fixed
  list in the agent configuration, no shell, arguments as a list, empty
  environment unless the caller names variables and a directory (Docker
  Compose reads its values from there), no stdin except for secrets
  (passwords never on the command line), timeout, output limit (output
  over it ends the program at once; what it says on stderr past its limit
  is read and dropped, so it never waits on its pipe), each in a process
  group of its own, killed whole on a timeout. A terminal in a container
  (`docker exec` through `setsid`) differs: it reads what is typed and has
  no timeout. Currently: `systemctl`, `systemd-run`,
  `systemd-mount`, `busctl`, `timedatectl`, `lsblk`, `smartctl`, `zpool`,
  `hostnamectl`, `zfs`, `sgdisk`, `wipefs`, `udevadm`, `umount`, `rmdir`, `rm`,
  `useradd`, `userdel`, `groupadd`, `groupdel`, `gpasswd`, `smbpasswd`,
  `testparm`, `exportfs`, `smbcontrol`, `sshd`, `docker`, `dockerd`,
  `setsid`, `openssl`, `nginx`, `nft`, `virsh`, `ssh-keygen`, `ssh-keyscan` (and, in transient units,
  `nut-scanner`, 4.16, `systemd-tmpfiles`, this binary as `copy-vm-iso`, `receive-vm-image`,
  `back-up-vm`, `restore-vm` and `remove-vm-backup`, 4.13, as `backup-unit`, which runs `rsync` and through it `ssh`, and `rclone`, 4.14, and `ip link
  delete` for a network device zendiloOS made that goes, 6).
  `zendiloos-docker-bridges.service`, a unit of the package, runs
  `iptables` and `ip6tables` as this binary's `docker-bridges` (4.12).
- **Reads** of fixed files where no program is needed: `/proc/mdstat`,
  DMI's `sys_vendor` and `product_name` (`/sys/class/dmi/id`), which name
  the machine for the enclosure (5), and an eMMC device's wear in SMART's
  place: the files of its card below `/sys/class/block/mmcblk<N>/device`
  (`type`, `life_time`, `pre_eol_info`, `name`, `serial`, `fwrev`), a few
  bytes each, for a whole disk the kernel has. Although they are world-readable, the
  agent provides them, so that all storage state comes from one place and
  the simulation can provide all of it consistently. For the same reason
  the list of block devices comes with udev's database of the whole disks
  (`udevadm info --export-db`, block devices that are disks): where each is
  plugged in, which `zendiloosd` cannot see in `/dev/disk/by-path`
  (`PrivateDevices`). Without an answer from udev the disks are still
  listed, only not placed by port.
- **Files** only at fixed locations known to the agent code, written
  atomically (temporary file, `fsync`, `rename`), for example
  `/etc/samba/zendiloos.conf`, `/etc/exports.d/zendiloos.exports`,
  `/etc/systemd/network/05-zendiloos-<interface>.network` and, for a
  bridge, bond or VLAN zendiloOS makes, `05-zendiloos-<name>.netdev`, with
  the record of a change of several of them beside
  (`zendiloos-change.json`, 6); taking the
  network over from ifupdown, also `/etc/network/interfaces` and
  `/etc/resolv.conf` (a link to systemd-resolved's stub), with the record
  of what they were in `/etc/network` (6). ifupdown's files are read only
  below `/etc/network`, never through a link.
- **Mounts** through generated systemd mount units that the agent asks
  systemd to start. The mount then lives in the host's namespace (the
  hardened agent has its own), dependencies on Samba/NFS are explicit, and
  mounting at boot needs no `fstab` edits. Removable drives get transient
  mount units (`systemd-mount`), bound to their device and gone at the
  next boot.
- **systemd** through `systemctl` with fixed arguments: `reboot` and
  `poweroff`, and enabling, starting, stopping and reloading the services
  zendiloOS manages (`smbd`, `nfs-server`, `wsdd2`, `avahi-daemon`, `ssh`,
  `docker`, `nginx`, `systemd-timesyncd`, `zendiloos-firewall`; for a
  network taken over from ifupdown `networking.service`,
  `systemd-networkd` and `systemd-resolved`; for a UPS Network UPS Tools'
  `nut-driver-enumerator`, `nut-driver@ups`, `nut-server` and
  `nut-monitor`, 4.16). systemd is reloaded only
  when something it reads changed, and then once: a unit file or drop-in
  the agent writes (written only then), or a unit's link. Every reload
  runs all generators again and is logged. Enabling and disabling the file
  services, SSH and zendiloOS's own mount and pin units goes with
  `--no-reload`; without it systemctl reloads systemd each time, also when
  no link changed, which made a change of the file services and every
  start of zendiloosd (which writes their configuration) reload it four
  times. Afterwards the agent asks systemd whether it is behind the disk
  (`NeedDaemonReload`; since systemd 257 every unit is, once a link
  changed without a reload, and `systemctl status` warns about it) and
  reloads if so. A new mount unit is linked before that reload and
  started after it, so its file and link take one. For the file services
  and SSH it also goes with `SYSTEMCTL_SKIP_SYSV=1`: smbd and ssh have
  init scripts too, and for those systemctl runs Debian's `update-rc.d`,
  which reloads systemd twice whatever `--no-reload` says; their rc.d
  links count for nothing beside the native units. What has
  to run outside the agent's sandbox runs as a unit: transient ones through
  `systemd-run` (apt, acme.sh, `nginx -t`, `nft -c`) and `systemd-mount`
  (removable drives), or a unit of the package. The kernel's NFS server
  keeps its export cache per network namespace, so releasing a share's
  exports before unmounting it starts `zendiloos-nfs-flush.service`
  (`exportfs -r`, `exportfs -f`). D-Bus is reached through `busctl`
  (systemd-networkd's `Reload`, `GetLinkByName`, `DescribeLink` and
  `ReconfigureLink`, a link's `AdministrativeState`) and `timedatectl`. systemd-networkd offers
  no way to delete a link over D-Bus and the agent has no network of the
  host's for rtnetlink: a device that goes is deleted in a transient unit
  (`systemd-run`, `ip link delete dev <name>`, with `CAP_NET_ADMIN` as its
  only capability), which runs in the host's namespace.
  **[Later]** zbus once operations need more, such as following a unit's
  state.
- **Interpretation happens unprivileged.** The agent returns raw tool output;
  parsing happens in `zendiloosd`. Exceptions are safety checks the agent
  must make itself (is this device the system disk? is it mounted?); for
  those it reads `/sys` and `/proc/self/mountinfo` directly and as narrowly
  as possible.

### 3.4 zendiloosd ↔ Linux (without the agent)

Read-only and unprivileged: `/proc` and `/sys` for metrics (`sysinfo` for
CPU, memory, processes, sensors; an own parser for `/proc/diskstats`), and
the journal for the log viewer. For the journal `zendiloosd` runs
`journalctl --output=json` through `zendiloos-exec`, with the
supplementary group `systemd-journal`. A request chooses among fixed views
(everything, zendiloOS, storage, file sharing, kernel), a minimum priority,
a page size and a cursor; nothing else reaches the command line.

The network is read with `ip -json` (addresses with `-details`, default
routes of both families), the only other program `zendiloosd` runs, with
fixed arguments; name servers come from `/etc/resolv.conf`, or from
systemd-resolved's own list when that file only names its stub. Reading
needs netlink but no capability, so `zendiloosd` may open `AF_NETLINK`
sockets. The agent cannot do this: it has a network namespace of its own.
Loopback, `veth`, `tun`/`tap` and Docker's bridges are left out.

A UPS's state is read over Network UPS Tools' own protocol (TCP, `LIST
VAR`), from NUT's server on this machine (127.0.0.1:3493) or from the
other device the UPS is connected to (4.16): nothing is run for it, and
the agent, without a network, could not ask.

---

## 4. Security and Permission Model

### 4.1 Threat Model

| Attacker | Goal | Main countermeasures |
|---|---|---|
| Device on the LAN without an account | log in, take over during initial setup | throttling, setup code (none on a machine installed from the ISO, whose setup is open to the own network only: **accepted**, as on other NAS systems, between installing and setting it up), no default password, TLS |
| Someone who has an account's password (phished, reused elsewhere, seen) | sign in to the web interface as that account | two-factor sign-in per account (4.5): a code from an authenticator app, or a recovery code, after the password; codes throttled like passwords. **Accepted:** SMB and NFS keep the password alone, as they have no second factor; the codes guard the web interface, where accounts and the system are changed |
| Signed-in user without admin rights | other users' files, privilege escalation | server-side permission checks: a share's rules for every operation, in zendiloosd and Samba, the files agent confined beneath the share's root; in the personal folder file access with the user's own kernel permissions (4.4) |
| Web page in the same browser | CSRF, WebSocket hijacking | `SameSite=Strict`, CSRF header, origin check including the WebSocket upgrade, strict CSP |
| Compromised `zendiloosd` | root | the agent offers only validated domain operations, no generic execution; system disk protected. **Accepted limit:** an administrator may run any Compose project and give an app the Docker socket (4.9), and the agent carries that out as asked, so whoever controls `zendiloosd` controls Docker and with it the machine; an administrator's account is as good as root, as on other NAS systems |
| Malicious container | host | Docker access only through agent policy (see 4.9) |

### 4.2 Accounts

**Decided: unified accounts.** One account is valid for the web interface,
SMB, the file manager, and later NFS and apps. "Admin" is a role, not a
separate account.

- The database is authoritative (role, groups, Argon2id hash for the web
  login, the second factor of the web login, preferences).
- For every account the agent creates a Linux account: UID from 2000, group
  `zendiloos-users`, shell `nologin`, no home directory, locked Linux
  password. What it writes in its personal folder belongs to it; in a
  share, what is written takes the owner of the folder it lands in and the
  group `zendiloos-files` (4.4).
- UIDs and GIDs are never reused. The agent keeps the highest ID it ever
  handed out in `/etc/zendiloos/id-high-water` and passes the next one to
  `useradd`/`groupadd` explicitly: files and ACL entries keep the number,
  so a new account must not inherit a deleted one's access.
- When a password is set, Samba receives its own hash (`smbpasswd` via
  stdin). The plaintext password exists only during that request.
- The agent refuses any change to accounts outside `zendiloos-users`. On an
  existing Debian system, system accounts and the owner's own user (UID 1000)
  stay untouched. "Managed" means: a user with a UID from 2000 to 59999
  whose primary group is `zendiloos-users`, or a group with a GID in that
  range. An app's account (`app-<id>`, with a group of its own) is
  neither: only the app's own requests touch it (4.9). Names the system
  already uses are refused (`name_taken`).
- Groups are Linux groups as well; the agent sets their members.
- Order of changes: the host first (where failures are most likely), then
  the database. A failed creation removes the Linux user or group again.
- Administrators cannot change their own role or access, delete
  themselves or reset their own password here (they use the account menu),
  and at least one enabled administrator always remains. Changes to
  accounts and groups are serialized in `zendiloosd`, so these checks cannot
  be raced.
- Accounts an administrator creates, or whose password an administrator
  sets, must choose their own password at the next login; until then the
  API answers everything else with `password_change_required`. Accounts
  whose password only administrators set (`password_locked`) are exempt
  and cannot change it themselves.
- **Password policy:** always at least 10 characters (up to 64 can be
  required), never containing the account name, never one of the most used
  passwords (an embedded list from Mark Burnett's public-domain ten
  million, those of at least 10 characters); composition rules (cases,
  digit, symbol) and a maximum age (30 to 730 days, checked at login) can
  be added, and are off by default, as NIST SP 800-63B advises.

Alternative: separate web accounts and share accounts. Simpler to build, but
unexpected for users, and the file manager would still need a mapping from
web account to file permissions.

### 4.3 Roles and Permissions

Handlers ask for a **permission**, never for a role. Roles are fixed and
bundle permissions, so the model can change in one place.

| Permission | admin | user | readonly |
|---|:-:|:-:|:-:|
| `system.view` (resource monitor, information) | ✓ | ✓ | ✓ |
| `system.manage` (settings, power, updates) | ✓ | | |
| `logs.view` | ✓ | | ✓ |
| `users.manage` | ✓ | | |
| `storage.view` | ✓ | | ✓ |
| `storage.manage` (create/destroy pools, wipe disks) | ✓ | | |
| `shares.manage` | ✓ | | |
| `files.read` (own shares in the file manager) | ✓ | ✓ | ✓ |
| `files.write` (change files in own shares) | ✓ | ✓ | |
| `containers.manage` | ✓ | | |
| `vms.manage` (virtual machines, their plugin and consoles) | ✓ | | |
| `backup.manage` (backup tasks, their plugin, log and restores) | ✓ | | |
| `account.self` (own password, sessions, preferences) | ✓ | ✓ | ✓ |

The frontend hides apps the user lacks the permission for; that is a
convenience. Enforcement happens only in the backend: the axum extractor
`Auth` gives the signed-in account, and the handler calls
`auth.require(Permission::…)`.

Everybody sees their own jobs (file transfers, deletions); those with
`storage.view` see all jobs, with what tools reported, and the
notifications (storage, folders, updates, certificates, the firewall).
The WebSocket topics `jobs` and `notifications` apply the same rules.

Notifications also go out on channels administrators (`system.manage`)
set up, so that a failing disk is noticed without the interface open.

- **Mail server.** One for all email channels (`notification_mail_server`,
  one row): SMTP over TLS, STARTTLS, or unencrypted to a relay in the own
  network, never with a password. The password lives in the database,
  which only `zendiloosd` reads; the API never returns it, only that one
  is stored. Left out of a change it stays, for the same server, port,
  encryption and user name only; another server needs it typed again, so
  a stored password never goes anywhere it was not typed for.
- **Channels** (at most 20): email to recipients, 1 to 10, each a
  zendiloOS account or an address. An account is mailed at the address
  it has when the mail goes out; one without an address, disabled or
  deleted gets nothing, and the channel says so per recipient. Webhooks
  send one HTTP POST, as JSON with the fields chat services, Gotify and
  home automation read, or as text with a `Title` header for ntfy. The
  messages are worded on the server in English or German as the
  interface words them, the NAS's name in the subject.
- **Routes.** Notifications come in kinds (`zendiloos_notify::routes::KINDS`),
  each a code, except a data check that found errors (`scrub_finished`
  as a warning or error), which is `scrub_errors`; grouped as storage,
  folders, system, certificates (the ACME client's automatic renewals,
  4.11) and security (a service the firewall blocks from everywhere,
  4.12), each with the most serious level it is raised at
  (a degraded pool is a warning, a faulted one an error: `pool_trouble`
  is an error kind). Per kind, administrators
  tick whether it is kept in zendiloOS and on which channels it goes out
  (`notification_routes`: a row per kind and channel, the channel NULL
  for zendiloOS). A kind not kept is not stored, listed or published to
  browsers, but still goes out where ticked. A kind nobody chose for
  follows the defaults: kept, and sent on a channel whose level (its
  former "sends everything / warnings and errors / errors only") carries
  that level. A new channel starts at warnings and errors; a
  change of a channel keeps its level; so new kinds reach existing
  channels as their level did before.
- **Delivery.** Each notification raised goes into a queue in the
  database (`notification_deliveries`, migration `0052`), once for each
  channel its kind is routed to, and out from there at once, each channel
  on a task of its own, with 30 seconds per try; the last outcome is kept
  and shown. A try that fails for a reason that may pass (the network or
  name resolution not up yet after a boot or an update, a connection
  refused, cut off or timed out, an SMTP 4xx, an HTTP 5xx, 408 or 429) is
  tried again after a pause that doubles from a minute to an hour, 18
  times in all, about twelve hours: counted in tries, not by the clock,
  which a machine without a battery for its clock sets only once the
  network is up. What cannot succeed fails for good at once: an SMTP 5xx
  (a password or an address refused), any other HTTP answer, a
  certificate not trusted, no mail server or no recipient with an
  address. A channel's notifications go out in the order raised: while
  the oldest waits, those behind it wait with it, and a new one for the
  channel, or a test that went out, tries them all at once. The queue
  outlives a restart of zendiloosd; a try cut off by one is made again,
  so a message may arrive twice but none is lost. The channel's card
  lists what waits (with the tries and the reason) and what failed for
  good, kept for a week, at most 20 per channel. A test goes to that
  channel alone and answers with the server's reason when it fails.
  Changes to the mail server, channels and routes are audited
  (`notify.mail_server_update`, `notify.channel_*`, `notify.routes_update`).
- **Before routes** (migration `0032_notification_routes`), each email
  channel carried its own server and a level. The first email channel's
  server became the mail server (another channel's different server is
  not kept: it sends through the first one's); email channels kept their
  recipients; no route is written, so each channel carries what it
  carried by its level. A tick is per kind, not per level, so a channel
  for errors only now gets all of a kind that can be an error: also a
  degraded pool, a disk's SMART getting worse, a volume 80 % full and
  updates installed, besides the faulted pool, the failing disk, the
  volume 90 % full and the missing ZFS module it got before; untick them
  to have fewer. It never misses an error it got before.

TLS is rustls with ring and the system's certificate authorities
(`ca-certificates`).

### 4.4 Share and File Permissions

- **Folders on a volume:** what lies directly on a volume is a share in
  this sense, whatever it is called on screen: its own dataset
  `<pool>/<name>` at `/volume<N>/<name>`, with its permissions, recycle
  bins, snapshots, quota, compression and record size. The file manager
  makes one with "new folder" on a volume (an administrator who manages
  shares; the creator may read and write, nobody else yet). Its rules
  hold for the folder as a whole, in the file manager as over SMB, with
  NFS clients named by computer and apps by what their Compose files
  mount. **Shared** is a flag of its own: only a shared folder
  goes into Samba's configuration and NFS's exports; one not shared is used
  on the NAS alone (a project's folder people may look into, say), keeps
  its NFS clients for when it is shared again, and loses nothing when
  shared or not. Time Machine needs one that is shared. Folders deeper
  down are plain folders and never shared on their own. Folders on a
  volume are not renamed: the dataset, its mount unit, Samba's and NFS's
  sections, the snapshots' names and whatever mounts its path (apps,
  projects, uploads under way) would all have to follow at once.
- **Access is the share's as a whole.** Its rules (per account or group:
  no rule, deny, read, read and write) live in the database, and nothing
  is written into its files per person: Samba enforces them in the share's
  section, `zendiloosd` for the file manager before anything reaches the
  files agent, and NFS names computers, not people (below). A change holds
  at once: Samba reads its configuration again (a reload, which open
  connections survive) and, where the change takes access away from anyone
  (a denial, reading instead of writing, a rule gone, the share off the
  network or deleted), the share's open SMB connections end (`smbcontrol
  smbd close-share`, after every smbd has read the new rules), so nobody
  keeps what they may no longer do; clients connect again under the new
  rules. No file changes and no job runs. A share nobody may use is not
  offered at all: Samba would read an empty `valid users` as "everybody".
  Administrators need a rule like everyone else.
- **Which rule wins:** a denial, the account's own or one of its groups';
  then the account's own rule, as it is more specific; then the broadest
  grant of its groups. Samba's section says the same: `valid users`
  (whoever may read or write), `write list` (whoever may write), `read only
  = yes` for the rest, and `invalid users` (whoever is denied), which wins
  over `valid users` and `write list`, also for a member of a group listed
  there. Groups stay groups (`@family`): Samba follows their members
  itself, on the next connection. A disabled account may do nothing
  anywhere: Samba has it disabled (`smbpasswd -d`) and denies it by name in
  every section whose rules would let it in, its own rule or a group's; it
  cannot sign in to the web interface either.
- **Samba works on the files as root.** Each share's section has `force
  user = root`, `force group = zendiloos-files`, `inherit owner = yes`,
  `create mask = 0660`, `directory mask = 2770` and `nt acl support = no`:
  the rules above are the only ones; what Samba writes takes the owner of
  the folder it lands in (an app's folder stays the app's) and the group
  `zendiloos-files`; Windows' security dialog changes nothing. The share
  `home` of the personal folders is the exception (below): there each user
  is themselves.
- **Following groups and accounts.** The Sharing context hears from the
  Identity context (`GroupObserver`, `AccountObserver`, bridged in
  infrastructure) after a group's members changed, and before and after an
  account is disabled, enabled again or deleted. Nothing in the files
  follows: Samba's configuration is written again, and where someone may do
  less now the share's open connections end: a member who left a group that
  grants, one who joined a group that is denied; a disabled or deleted
  account on every share it could use, as noted before the change, while
  its rules and groups are still there. The change itself stands whatever
  the Sharing context makes of it. When a group is deleted, its rules go
  first, as a change of each share's rules.
- **On disk every share is alike.** The package makes the system user and
  group `zendiloos-files` (sysusers; no account of a person). A share's
  folder is `root:zendiloos-files`, mode 2770, with one ACL set when it is
  made: `group:zendiloos-files:rwx` and `default:group:zendiloos-files:rwx`,
  mask `rwx`, which what is made in it inherits; its recycle bins' folder
  `.recycle` alike, and sticky (3770), so only root, which the file
  manager works as, renames or removes the bins in it. Whoever works on
  the files as that group (NFS clients, apps) reads and writes what Samba,
  the file manager or another app made. There are no entries per person or
  group, so nothing has to follow rules, group members or accounts.
- **Personal folders:** one dataset `<pool>/zendiloos.homes` on one pool,
  mounted at `/volume<N>/@homes` (root, 0711); each account given one has
  `@homes/<name>` (its own, 0700) and a ZFS `userquota`. Samba offers them
  as one share `home` (`path = …/@homes/%U`, `valid users` the accounts
  given one), with a recycle bin like the shares; the name `home` is
  therefore no longer free for shares. In the file manager it is share 0,
  which the files agent resolves to the folder of the user it acts as and
  nobody else's. Those who manage shares see every account's personal
  folder listed (by name, in `GET /files/volumes` with the volume holding
  them), only their own to open; nothing changes in how the others are
  reached, and everyone else learns no other account's name. Turning them
  off, or taking one away, leaves the files.
  A folder of the name that belongs to another UID (an earlier account of
  the same name) is moved aside (`<name>.previous-<uid>`) and a fresh one
  made, so the new account is never locked out of its own folder.
- **The update to share-level access** walks each share made before it
  once (migration `0043_share_level_access`; `shares.permissions_migrated_at`
  is when its files got the one ACL, at creation for a share made since).
  Shares made before had their rules written into every file as entries
  per person and group. The walk sets the share's folder and `.recycle` as
  above (owner, group, mode and ACL, the named entries of people and their
  groups gone there too), then gives every folder and file below the
  group's entry (folders `rwx` and the default entry, files `rw` and `x`
  only where somebody may execute them already) and takes the named
  entries of the IDs zendiloOS hands out to people and their groups (2000
  to 59999, those deleted since among them, and `zendiloos-users`, every
  account's primary group). The apps' accounts and groups (`app-<id>`, see
  4.9) and IDs zendiloOS does not hand out (system accounts, the owner's
  own user, the users containers run as) keep their entries, so installed
  apps keep working; owners stay as they are. The files agent walks by
  descriptor, depth first (`O_NOFOLLOW`, only the folders on the way down
  open, each read as it goes, never held whole: a folder of millions of
  entries takes no more memory than a small one), and changes each ACL
  through its descriptor (`system.posix_acl_*` via `/proc/self/fd`, the
  edits of `setfacl -x/-m` with the mask recomputed), so a folder swapped
  for a link meanwhile changes nothing; links, other file systems and what
  is neither folder nor file are left alone. An entry that cannot be
  changed, or a folder nested deeper than 256, does not stop the others:
  the walk goes on to the end and fails then, counting them and naming one
  of each. Each walk is a job (`sharing.migrate_permissions`, the share's
  name as its target, its output how many entries it went through), one at
  a time on the whole NAS and one share after another, when zendiloosd
  starts and every five minutes; a share whose walk failed or was cut off,
  or whose volume was not mounted, stays owed and is walked later.
  Meanwhile its rules hold and may change, as Samba and the file manager
  do not depend on the files; NFS clients and apps reach what the walk got
  to. A rollback or the deletion of the share waits for its walk ("try
  again once that has ended"). Verified in the test VM from data with
  named ACLs (`just vm-permissions-test`).
- **Rolling back** a share to a snapshot (5) brings back its files as they
  were then; the rules hold as recorded now, and the share's folder gets
  what every share's has again. A snapshot from before the share's walk
  brings back the entries per person of then: the share owes its walk
  again, which starts at once (the answer's `job`). A walk records the
  second after it ended, so a snapshot taken while it went counts as
  older.
- **Recycle bin:** the file manager moves what is deleted in a share to
  `.recycle/<user>/<folders>/<name>` there (`Copy #N of <name>` if taken),
  if the share's recycle bin is on; what is deleted over SMB or NFS is gone,
  and the snapshots keep earlier states (Windows' "Previous Versions").
  Samba keeps no bin in a share: it works there as root, and its
  `vfs_recycle` makes the bins' folders by path, so a link that whoever
  may write the share (an NFS client, an app) put in `.recycle`'s place,
  or in a user's place in it, would have root make folders wherever it
  leads. `.recycle` is hidden over SMB but not vetoed. One bin per share:
  the folder of each user says who deleted what, and everyone who may use
  the share sees all of it, as they could see the files before (over SMB
  with hidden files shown, in the file manager by showing hidden entries);
  whoever may write the share may move things out of it. The agent makes
  `.recycle` with the share's folder (above), and moves something not a
  folder in its place aside (`.recycle.not-a-folder-…`). The file manager
  reaches the bins beneath the share's folder only (the user's folder there
  made 0770, the share's group's); inside a bin, deleting is for good, at
  any depth. Its recycle bin view gathers the person's own folder of each
  share's bin, and restores from it, any entry at any depth: back to where
  it was deleted from (`keeptree` keeps the folders), without the `Copy #N
  of ` of a second deletion, of the entry or of a folder on its way (what
  is inside a folder deleted twice goes back into that folder), folders
  that are gone made again, and a taken name gets a free one beside it;
  emptying the bin deletes `.recycle/<user>` for good. In the personal
  folders each user's bin stays their own (0700), and Samba keeps it too:
  there it works as the user, whom only the user's own links can lead.
- **File manager: zendiloosd checks, the files agent stays beneath the
  root.** `zendiloosd` cannot read shares itself. Every operation goes to
  the files agent, which runs it on a new thread with the rights of its
  place, which end with the operation:
  - in a personal folder the signed-in user's credentials (`setgroups`,
    `setresgid`, `setresuid` as raw system calls, which on Linux change
    only the calling thread; the thread then has no capabilities left,
    which the agent checks): the kernel decides, as over SMB;
  - on a share root's access to files (`CAP_DAC_OVERRIDE`,
    `CAP_DAC_READ_SEARCH`) and `CAP_CHOWN`, every other capability dropped,
    once `zendiloosd` has checked the share's rule for that operation, from
    the database: a denial (or no rule) keeps the person out of the share
    altogether, reading allows what only reads (folders, search,
    properties, downloads, archives, snapshots), and only reading and
    writing allows a change (uploads, new folders, renames, copies and
    moves into it, deletions, restores, copies back from a snapshot); a
    reader's folders are listed as not writable. What it makes there takes
    the owner of the folder it is made in and the group `zendiloos-files`
    (through the new entry's descriptor, after its times are set), as
    Samba's does; only the folders an administrator's project binds are the
    administrator's, whose account its containers run as (a database
    refuses a data folder of another owner);
  - on a removable drive root's access to files and nothing else
    (`CAP_DAC_OVERRIDE` and `CAP_DAC_READ_SEARCH` effective): a drive
    formatted elsewhere has owners nobody here has, and only an
    administrator's request names one; its root is opened only while a file
    system of its own is mounted there, never the tmpfs beneath.

  Only managed users are named (UID 2000 to 59999, primary group
  `zendiloos-users`), never root or system accounts. A copy or move
  between places of different rights (a drive or the personal folder and a
  share) runs on two threads, each with its side's rights: one walks the
  source as a copy does and sends what it finds in order, files already
  opened (at most 32 pieces on their way), the other makes the copy from
  them, so what lands in a share takes its folder's owner and group, what
  lands in the personal folder belongs to the person, and only what the
  person may read there reaches a drive; a move removes the source once the
  copy is complete and written out, and only what was copied (see Moves
  below).
  Paths are resolved
  relative to the share's root with
  `openat2(RESOLVE_BENEATH | RESOLVE_NO_XDEV | RESOLVE_NO_MAGICLINKS)`, so
  neither `..` nor a symbolic link leads out of the share; copying and
  removing trees goes name by name from directory descriptors and never
  follows a link. New names (uploads, new folders, renames) must not
  contain control characters, begin or end with a space or end with a
  dot, which Windows could not reach; files written over SMB or NFS keep
  their names and stay reachable. A name that is not UTF-8 (written in
  another encoding) is listed as it looks, with replacement characters,
  and marked: the interface does nothing with it, since a request with
  that text would reach another entry of exactly that name; it is renamed
  over SMB or NFS. For downloads and uploads the agent passes the opened
  file back via `SCM_RIGHTS` and `zendiloosd` streams the data; the root
  process never handles file contents. An upload is written to a hidden
  file in the target folder and renamed when complete, so a broken upload
  never replaces a file. One given up is removed only as a file, never a
  folder: whoever may write the folder could rename one of the user's
  folders to the upload's name. zendiloosd records each upload under way
  (`pending_uploads`); what one left when zendiloosd stopped in the middle
  is removed when it starts again, as that user. A failed copy removes
  only what it made, taken by the descriptor it made it through (a link
  only while a link has the name); folders are emptied in batches, never
  read whole. Copies leave uploads under way out (a half-written file
  that nothing would ever remove there). New files in a share take the
  folder's owner, the group `zendiloos-files` and the share's inherited
  ACL, exactly as over SMB; in the personal folder they are the user's.
  Copies, moves into another place and
  deletions run as jobs; a rename is a quick request. Verified end to end
  in the test VM (`just vm-sharing-test`).
- **Moves into another place** (another share, the personal folder, a
  drive: another file system) copy, then remove what was copied, and
  nothing else. The copy records each entry it copied as it goes, a
  compact tree in memory: name, device and inode, kind and change time,
  for a file also its modification time and size, for a link its target,
  all from the descriptor it copied through, taken before it read. Device
  and inode alone do not tell an entry: ext4 gives a new entry the number
  of one just removed, and ZFS reuses object numbers too; what is put in a
  copied entry's place has a change time of its own, and a link its own
  target. FAT and exFAT have no inode numbers: the kernel numbers an entry
  anew whenever it forgot it and reads it again, which a big move off a
  card makes it do, and shows the modification time as the change time
  then; a source on them (`statfs` type) is told by name, kind, size and
  modification time alone. Times move in steps (a clock tick, two seconds
  on FAT), so an entry that changed within a step before the copy looked
  at it is looked at again once the step passed; one that keeps changing
  stays. Before anything is
  removed, the target's file system is written out (`syncfs` on the
  target's root, on the target's side, through a descriptor opened before
  the copy: the kernel tells a write-back error only to descriptors opened
  before it, and one opened after somebody else was told of it misses
  it); if that fails, the source stays
  and the move says so: a drive pulled out after "done" would otherwise
  lose both. Removing then walks the record, never the folders: what is
  missing by then is passed by, but for the moved entry itself, which,
  renamed or removed meanwhile, was not moved and is counted with all of
  it; what changed since (another entry under
  the name, a file written to, which its change time or size tells; FAT
  keeps times in two-second steps, hence the size) stays and is counted;
  files and links that are still what was copied go; a folder goes once
  that left it empty, otherwise it stays and is counted (something new
  came into it, an upload under way, a pipe, a file system mounted inside,
  none of which are copied). A folder's change time is compared before
  what is in it goes, which changes it too: one that changed since it was
  copied (something came, went or was renamed in it, or another, empty
  folder took its place and number) stays once what in it is still as
  copied went; a folder that stays without being looked into counts with
  everything of it the record holds. A move that left anything is never a plain
  success: the job ends as `partly_moved`, with the number of entries that
  stayed and one of them, which the explorer shows. Tried again, the move
  goes on into its earlier copy, a folder of the name in the target: it
  adds what is missing there, counts a file or link the target has under
  the same name as moved if it is the same, byte for byte, and otherwise
  leaves the source's where it is, never replacing the target's. The record has a
  budget like an archive's plan (half a million entries, 16 MiB of names,
  some 50 MB of the files agent's memory at most): a
  move of more keeps the complete copy and the whole source and says so
  (`copied_not_moved`). A name of the source that the target takes for
  another one, as FAT, exFAT and NTFS do with names that differ only in
  case, fails the copy with both names, not as a name "taken meanwhile".
  A plain copy onto a drive is written out the same way before it counts
  as done.
- **Downloads and previews.** A download answers one byte range
  (`Range`, 206 or 416), so browsers resume a broken download (`If-Range`
  with the `Last-Modified` it was given; a file changed since comes again
  whole) and videos can be wound. A download is an attachment, except
  that pictures, sound, video and plain text may be shown in the browser
  (`inline=true`), by a fixed list of extensions and with that type; HTML,
  SVG, PDF and anything else is only ever saved, because a file shown from
  our origin could run script there. Every download also carries
  `Content-Security-Policy: sandbox; default-src 'none'` and `nosniff`.
- **Search.** The explorer's search looks through the folder shown and
  everything below it on the NAS (`GET /files/shares/{id}/search`, and
  `/files/removable/{drive}/search`, with `path`, `text`, `hidden` and in
  a snapshot `snapshot`): names containing the text, whatever the case.
  The files agent walks with the rights of the place (above), like the
  other walks: by descriptor, name by name, never following a link, never
  onto another file system, no deeper than 256 folders, each folder checked
  to be the one looked at before it is entered; what it may not read is
  passed by. Names starting with `.` only when hidden entries are shown,
  the recycle bins only when searched from inside one, uploads under way
  never, and a name that is not UTF-8 is neither found nor entered (it
  could not be told from one that looks alike). It answers with at most
  500 entries within eight seconds, saying so when it stopped early, and
  stops as soon as `zendiloosd` hangs up (the person typed on, or left).
- **Properties.** What a file or folder is comes from the NAS
  (`GET /files/shares/{id}/facts`, `/files/removable/{drive}/facts`, with
  `path` and in a snapshot `snapshot`): a file or a folder, when it was
  made (statx's birth time, where the file system keeps it; for a folder
  on a volume the time it was created in zendiloOS otherwise) and last
  changed, its owner's account, a file's size; for a folder its files,
  folders and hidden entries below it and what the files hold, counted by
  the files agent walking as the search does.
  Hidden entries (a leading `.`) are counted, not entered, so the recycle
  bins are not counted through; uploads under way are not counted. At
  most ten million entries within half a minute, saying so when it
  stopped early with what was counted by then; the explorer asks while a
  properties dialog is open and calls it off when the dialog closes, which
  stops the walk on the NAS too. A folder directly on a volume has four
  tabs: general (name, location and how computers reach it, `\\<host>\<name>`
  over SMB and `<host>:/volume<N>/<name>` over NFS, with copy buttons; the
  space it uses, its quota, what it holds as counted, times, owner,
  description, recycle bin, compression and record size), permissions (a
  table of every account and group, each with deny, read and write, or
  read only, one at most, and a column with what holds for each account
  with its groups taken into account, as the NAS decides it: a denial
  wins, then the account's own rule, then the broadest rule of its groups;
  groups open to their members), sharing (on the network or not) and NFS
  (its rules, each added and changed in a dialog of its own).
- **Folders as archives.** A folder downloads as a tar archive (POSIX
  pax: long names whole, links as links), which Windows, macOS and Linux
  open without extra software. The files agent first plans it with the
  rights of the place: every entry and its size, so the archive's exact length is the
  `Content-Length`, and anything that cannot be read refuses the download
  instead of leaving files out silently. It then writes the archive the
  same way into a pipe whose reading end goes to `zendiloosd` like a
  downloaded file, following the plan to the byte (a file that grew is
  cut, one that shrank padded); should writing fail midway, the download
  ends short of its length and the browser reports it as failed. What was
  sent cannot be taken back, and a download that looks complete must be
  the folder as it was: an entry that changed while the archive was
  written (another size than planned, written to while it was read, gone,
  a link pointing elsewhere) is written as planned, and the archive is then
  left without the two blocks that end it, so it ends short all the same;
  the files agent logs how many changed and the first of them. A file
  rewritten to its planned size before it is read goes in as it is then. At the
  share's top the recycle bins are left out, everywhere uploads in
  progress; at most four folders are written at once.
- **The explorer** is one app for files and for the folders on the
  volumes (the Shares app of before is part of it): on the left a tree of
  places (the personal folder, the favourites, each volume with its folders
  down to any depth and, for those who manage shares, under the volume
  holding them a node "Personal folders" with every account's by name: their
  own opens as the personal folder does, the others' show a lock and, opened,
  say kindly that they are private, without asking the NAS; the removable
  drives plugged in, and one recycle bin
  view that gathers the person's bins of every folder, each entry with the
  folder it was deleted from; folders in it open to any depth, and what is
  in them is restored or deleted for good from there too), on the right the
  details view (name, modified, type, size; sorted; chosen with clicks,
  Ctrl, Shift and Ctrl+A). What is typed into the address bar's search
  filters what is shown at once; in a folder, the NAS then looks through
  everything below it (see Search above), and what it found takes its
  place, each entry with the folder it is in and "Open its folder", which
  goes there with the entry chosen; Escape or "Clear the search" goes back.
  The context menu (a right click, or a long press on a touch screen, on an
  entry, on empty space or on a node of the tree) offers the actions; there
  is no toolbar. One list says which belong where the explorer is and which
  the choice allows: open, download, new folder, upload, cut, copy, paste,
  rename, delete, restore and empty the recycle bin, copy back from a
  snapshot, favourites, for a folder directly on a volume share on the
  network or stop, previous versions (snapshots looked into, and for those
  who manage shares taken, deleted and rolled back), eject and mount a
  drive, and properties (general, permissions, sharing, NFS). Keys
  do the same (Del, F2, Ctrl+C/X/V/A, Enter, Backspace and Alt+arrows);
  entries dragged onto a folder of the same place move, onto another place
  (another folder on a volume, the personal folder, a drive) they are
  copied, so nothing leaves a place by a slip of the mouse; Ctrl (or Alt)
  copies and Shift moves either way, and the pointer and the folder say
  which while dragging. Files dragged from the computer upload. The volumes come from `GET /files/volumes`: someone who
  manages shares sees every folder on them, those they may not open
  marked, and makes new ones ("new folder" on a volume: off the network,
  for its maker only); everyone else sees only the folders open to them.
  On a phone the tree is a drawer; a node lights up while something is
  dragged over it.
- **Removable drives in the explorer**, for those who manage storage
  (`storage.manage`), from `GET /storage/removable`, asked every four
  seconds while the explorer is shown, so a drive plugged in appears and is
  mounted by itself. A mounted drive is a place like a folder on a volume,
  under `/files/removable/{drive}/…` (folder, download, archive, upload,
  folders, rename, transfer, delete): listed, previewed, downloaded,
  uploaded to, copied and moved within and to and from folders on volumes
  (`transfer` takes `target_share` or `target_drive`), deleted for good
  (a drive has no recycle bin, no snapshots and no favourites, and is never
  shared). Copies and moves onto a drive are written out (`syncfs`) before
  they count as done, and a move removes its source only then. nginx
  passes uploads to a drive and downloads and archives from one on as they
  come, of any size and for up to an hour, as for shares, instead of
  holding them on the system disk. `zendiloosd` lets only someone who manages storage name a drive,
  and only one the disks show mounted at its place now (the Storage
  context's `RemovableDrives::mounted`, through a port of the Sharing
  context); write-protected, it is read only. "Eject" (the context menu of
  the drive in the tree, or of empty space in it) unmounts and powers it
  off, saying so when files on it are still open (`drive_in_use`); an
  ejected drive, or
  one that could not be mounted (its reason shown), offers "Mount". A
  drive ejected or pulled out while looked into sends the explorer back
  to where it starts.
- **Favourites** are folders an account keeps at hand, in its order (at
  most 50, in the database with the account and the share). Only those in
  shares the account may use are shown; the others are kept, should
  access come back.
- **What share-level access gives up.** For shares, zendiloOS no longer
  lets the kernel decide what a person may do: a gap in zendiloosd's
  checks, or in the files agent's confinement beneath a share's root, would
  give someone more than their rule. "The kernel checks, not our code"
  still holds for the personal folders. It was given up for shares
  because the entries per file only ever protected NFS: Samba and the file
  manager check the rules themselves anyway, and NFS with `sec=sys`
  believes whatever user a client claims to be, so a person's rule over NFS
  was never a boundary. Keeping every file's ACL in step with the rules,
  the groups' members and the accounts took a walk through whole shares
  after every change (for rules, members, disabled, enabled and deleted
  accounts, rollbacks), with owners handed over to root, revocations owed
  until a walk had ended without errors, and backlogs that could take
  hours; that machinery was the source of most of what went wrong. What
  protects instead: the rules, checked for every operation in zendiloosd
  from the database before the files agent hears of it (API tests cover
  each role), and the same rules in Samba's sections; the files agent's
  confinement beneath the share's root (`openat2` with `RESOLVE_BENEATH`,
  names walked by descriptor, never following a link, never onto another
  file system), on threads that keep root's access to files and
  `CAP_CHOWN` and nothing else; and NFS limited to the computers an
  administrator names. The alternative was to keep per-file ACLs and the
  file manager acting as the user, with the walks above.
- **NFS is per computer, not per person.** NFS (v3 and v4, `sec=sys`)
  believes whatever user a client claims to be, so a rule names computers:
  an IP address or network, a host name or a pattern with `*` and `?` as
  `exportfs` reads them (`*.example.com`; lowercase letters, digits, `-`
  and `.` only, checked by `zendiloosd` and again by the agent, so nothing
  reaches the exports file that it would read as a separator or an
  option), and says how: `rw` or `ro`; `sync` or `async` (writes answered
  before they are on disk); `secure` or `insecure` (ports above 1024, which
  any user of the client may open, for clients such as macOS's Finder).
  Every request is squashed to `zendiloos-files` (`all_squash`, with the
  user's and group's IDs, which the agent reads itself): whoever is on a
  named client has the share's read, or read and write, access, whatever
  user they claim to be, root too, as `sec=sys` gave in effect before; the
  interface says so. The line the agent writes per client is
  `<host>(rw|ro,sync|async,no_subtree_check,all_squash,anonuid=<uid>,anongid=<gid>,[insecure,]sec=sys)`.
  Rules from before, which chose whose identity the requests keep
  (`root_squash`, `no_root_squash`, `all_squash`), are all squashed since
  the update (migration `0043`). **[Later]** Kerberos.
- **Creating and deleting a share keep its record and its dataset in
  step.** A share that cannot be applied after it was stored goes again,
  record and dataset. Deleting hides the share first (it is offered no
  more and its name stays taken) and forgets it only once its dataset is
  gone; if applying fails, it is back as it was, and data that cannot go
  yet (busy, or zendiloosd stopped) goes when zendiloosd starts again.

### 4.5 Login, Sessions, CSRF

- Passwords: Argon2id (19 MiB, 2 passes). At least 10 characters, at most
  1024 bytes, no control characters, not one of the most common passwords,
  and not containing the account name (a name of four characters or more;
  a shorter one only as the whole password). On the accounts page an
  administrator may require more: a longer minimum (up to 64), upper and
  lower case, a digit, a symbol, and a change after 30 to 730 days, asked
  for at the next login. Unknown account
  names cost as much time as wrong passwords, so response times do not
  reveal which accounts exist. A disabled account is answered as a wrong
  password (`invalid_credentials`, in the same time, and counted as a
  failure), so a sign-in does not tell that its password was right: it is
  decided on the account as read before the password check, with no more
  reads of the database than a wrong password takes. The audit log records
  it as denied for being disabled.
- Sessions: 256-bit random token in a cookie (`HttpOnly`, `SameSite=Strict`,
  `Path=/`; with secure cookies also `Secure` and the `__Host-` prefix). The
  cookie has no `Max-Age` and ends with the browser session. The database
  stores only the token's SHA-256 hash. Sessions end after 30 minutes idle
  (set from 5 to 1440 under Security) and 12 hours at most; a password
  change ends all other sessions. A login whose password check overlapped
  a change or reset of the password, or the account being disabled or
  deleted, starts no session: it reads the account again under the lock
  those changes hold and needs the hash it checked. Idle means nobody used the page: reads it
  makes on its own after two minutes without input (polling, refreshing
  windows) carry `X-Zendiloos-Background` and do not count, nor does the
  live connection's check; a running upload counts, every five minutes. Administrators see every account's
  sessions with address and browser, and end any of them; the latest
  logins come from the audit log.
- CSRF: per-session token in the `X-CSRF-Token` header on every
  state-changing request, compared in constant time. In addition,
  `Sec-Fetch-Site` must be `same-origin` or `none` and `Origin` must match
  `Host` when the browser sends them; login and setup check this too, so a
  foreign page cannot sign a victim into an attacker's account. WebSocket
  upgrades (`/api/v1/ws`, a container's terminal) are GETs without a CSRF
  token; they get the `Sec-Fetch-Site` and `Origin` checks.
- Throttling in the backend, in memory: 20 failures per address within 15
  minutes, and the account lock as set under Security (5 failures within 15
  minutes by default, or off), for passwords and setup codes alike. An
  attempt counts as failed from its start until it ends otherwise, so
  attempts sent at once get no more checks than attempts one after
  another. Plus `limit_req` in nginx for login, the second sign-in step, setup,
  password changes and the two-factor changes (also confirming a setup).
- **Blocked addresses:** an address with too many failed logins (10 within
  5 minutes by default) is blocked from the whole API for a day, or until
  lifted; administrators block addresses and networks by hand too. Every
  request is checked in memory before it reaches a handler; only the health
  check stays open. Allowed addresses and networks, and this machine
  itself, are never blocked, and an administrator cannot block the address
  they use. Blocks are stored (`blocked_addresses`) and survive restarts;
  they apply to the web interface only, not to SSH or SMB.
- **TLS:** nginx accepts TLS 1.2 and 1.3, or 1.3 alone, as set under
  Security; the agent writes `/etc/zendiloos/nginx/tls.conf`, which the
  site includes, and reloads nginx. CSRF protection, the
  Content-Security-Policy and the rate limits are always on. At most two
  Argon2 runs at once (19 MiB each); the rest wait, so a flood of requests
  costs time, not all memory. A new password is checked before the
  current one is verified.
- **Initial setup:** as long as no admin exists, the interface shows a setup
  wizard. It requires a one-time **setup code**, stored in
  `/var/lib/zendiloos/setup-code` (0600), logged to the journal as a
  warning and shown on the console (HDMI/serial). Nobody on the LAN can
  complete the setup before the owner. The first administrator is created
  atomically, so two concurrent setups cannot both succeed.
- **Open setup (installed from the ISO):** the ISO's installer leaves the
  marker `/var/lib/zendiloos/setup-open`. While it exists and no admin
  does, no code is made: the console says to open the address in a browser,
  the setup page asks only for a name and a password, and whoever is first
  becomes the administrator. Only from the NAS's own network: the address
  nginx passes in `X-Real-IP` (its `$remote_addr`, set for every location,
  never taken from the client; zendiloosd trusts it only from nginx's
  socket or loopback) must be loopback, private IPv4 (10/8, 172.16/12,
  192.168/16), link-local, the shared range 100.64/10 (Tailscale and
  similar VPNs), or IPv6 unique local (fc00::/7) or link-local (fe80::/10),
  IPv4-mapped forms included; anything else gets `setup_not_local` (403).
  A NAS behind another reverse proxy of the LAN sees that proxy's address:
  set it up before publishing it. Throttling applies as for codes; the
  audit log records `open_setup`, or `open_setup_from_elsewhere` for a
  refusal. The setup, or an admin found at startup, removes the marker. A
  NAS installed with apt on an existing Debian (possibly reachable from the
  internet) never gets the marker and keeps the code.
- **Two-factor sign-in (TOTP):** each account can turn it on from its
  account menu; it applies to the web interface only, not to SMB or NFS.
  RFC 6238 as authenticator apps compute it: HMAC-SHA-1, six digits,
  30-second steps, one step of drift either way, a 160-bit secret from the
  operating system's random source, shown as an `otpauth://totp/zendiloOS:
  <name>?secret=…&issuer=zendiloOS` QR code and as base32 for typing in.
  Setting it up takes the account's password, so a session alone (left
  open, taken over) cannot turn it on with someone else's phone and lock
  the owner out; a wrong password counts against the account lock and the
  address block like a failed login (`wrong_password`). Not for an account
  whose password an administrator sets (`password_locked`): it may be
  shared, and one of those sharing it would lock the others out. A secret
  being set up waits for its first code, which turns it on; until then the
  password alone signs in. It waits ten minutes, for the session that began
  it, with the password it was begun with (changed or reset, it ends), in
  memory only (a restart ends it); three wrong codes end it, each audited
  (`wrong_code`), and it is begun again with the password, so a session
  taken over cannot guess the first code of a setup its owner left. The time step of the last code taken is
  stored, and a code of that step or an earlier one is refused, so no code
  signs in twice, not even two sign-ins at once (the check and the update
  run under the lock of account changes). Ten recovery codes (ten
  characters from 31 that cannot be mistaken for one another, about 49
  bits each) come with it, are shown once and kept as SHA-256 digests; each
  signs in once, and new ones (for the password and a code from the app)
  replace the old.
  The secret is stored as it is in `zendiloosd`'s private database
  (`account_two_factor`, 0600), as the server must compute codes from it;
  whoever reads that database has the secrets as well as the digests, which
  is why the digests need no salt.
- **Sign-in with a second factor:** the right password starts no session.
  `POST /auth/login` answers 202 with a random token (256 bits, as for
  sessions; only its hash is kept), and the sign-in waits in memory for at
  most five minutes, bound to the account and to the address the password
  came from (5 at most per account and 20 per network, an IPv4 address or
  an IPv6 /64, each making room among its own, so nobody ends others'
  waiting sign-ins; 1000 in all; the oldest go first). The code goes to
  `POST /auth/login/second-factor` with the token. A wrong code counts
  against the account lock and the address block like a wrong password,
  and the password step does not clear the account's failures, so knowing
  the password gives no more guesses; three wrong codes end the sign-in,
  the third answering `sign_in_expired`. Whatever the account lock is set
  to, an account takes at most 10 wrong codes from the app an hour and 30 a
  day, from wherever they come (also when turning it off or renewing
  recovery codes), and a sign-in that succeeds does not reset that: someone
  with the password and many addresses does not get to guess the six digits.
  Recovery codes are not counted there, or someone who knows the password
  could keep the owner from them for a day by sending wrong codes; guessing
  one is hopeless anyway (ten of about 49 bits each, three tries a password
  step, which takes an Argon2 run, at most two at once), and the address's
  limit and block hold them as they hold passwords, as does the account lock
  where it is set. Administrators clear an account's failed sign-ins on the
  users page (the account lock's count and the limit of codes from the app,
  audited as `identity.failed_sign_ins_clear`); the counts are kept in
  memory, so restarting zendiloosd on the console clears them too.
  Under the lock of account changes the second step checks the sign-in
  still waits (sent twice at once, it starts one session) and reads the
  account and its second factor again: a password changed or reset, a
  second factor reset or set up anew, or the account deleted meanwhile end
  the sign-in (`sign_in_expired`), a disabled account is refused. Turning it off takes
  the password and a code (or a recovery code); administrators reset it for
  someone who lost their phone and their recovery codes (audited,
  `identity.two_factor_reset`), and the users page shows who has it on.
  Turning it on or off, a reset and new recovery codes are audited; every
  one ends the account's other sessions (a reset all of them) and its
  waiting sign-ins. A restart of `zendiloosd` ends waiting sign-ins too.
- **[Later]** passkeys.

### 4.6 Agent Hardening

- A closed set of operations; no "run this command", no paths from the API
  that the agent does not check again against fixed roots.
- Operations that walk whole trees or disks (copying, moving and deleting
  trees, the one walk of a share to share-level access, preparing and
  destroying pools,
  fetching and running apps and projects, moving Docker's data) have two
  places of their own; quick ones have four. A move within one place (a
  folder on a volume, the personal folder, a drive) is a rename and quick,
  so renames and the last step of an upload never wait behind hours of
  copying; only moves into another place are long. One user's large copies
  never keep others from listing, downloading or uploading. Each user has one
  long place at a time (a second copy waits behind the first, not in a
  place of everybody's), the walks to share-level access have two of
  their own, backups and restores of Docker's volumes (hours in units of
  their own) two more, counting the volumes' data one, and a folder
  download read at less than a mebibyte in five minutes is given up. The
  agent may hold 65536 descriptors.
- **Nothing happens behind a caller's back:** a request still waiting for
  its turn, a place or a lock another operation holds, when its client
  gives up (zendiloosd's own timeout, its connection closed) is dropped,
  never carried out later. The client says how long it waits (`wait_ms`);
  the agent waits for the turn half of that at most, so what was asked has
  time to run and be answered, and refuses what has not started once it is
  asked to stop; what has started is seen through (`KillMode=mixed`: the
  programs it runs are not stopped with it, `TimeoutStopSec=2h`). zendiloosd waits two minutes for a quick system request
  and two hours for a long one, ten minutes and a day for files requests
  (copies, moves, the walk of a share), so once begun, a change is
  seen through and its outcome reported. Docker's work (fetching images,
  making containers, a deploy) has one budget of 30 minutes per request,
  a deploy's pull and its containers together (`DOCKER_WORK`), and 20
  minutes more at most for the units that pin its folders and the looks
  at what its containers mount; zendiloosd waits twice their sum
  (`DOCKER_PATIENCE`, 100 minutes), so the agent's half for a turn and the
  work both fit before it gives up.
- Every change to a pool, scrubs included, requires its `zendiloos:volume`
  property: pools the owner made stay untouched.
- Known limits: with `erasable = "unused"`, a compromised `zendiloosd` could
  have any disk erased that nothing uses, including an exported pool or a
  cold backup; list the erasable disks on machines where that matters. On
  an existing Debian, any process running as `www-data` can reach the API
  socket and claim an address in `X-Real-IP`, passing by the per-address
  throttle.
- Recycle bins: a share's `.recycle` is made with the share's folder,
  root's and the group `zendiloos-files`', sticky (3770 with the share's
  ACL); the file manager makes each user's folder in it as root, so
  whoever reaches the files as that group (an NFS client, an app) may
  neither rename nor remove a bin. Samba, which works on shares as root,
  keeps no bin there (its module follows links by path). Something not a folder in
  `.recycle`'s place is moved aside. In a personal folder the file manager
  refuses a bin that is not the user's own and private, and empties a bin
  without removing it.
- Making a disk blink (5, enclosure) reads and never writes:
  `read_burst` takes only a whole disk the agent finds itself, as for
  SMART (a validated kernel name, its entry in `/sys/class/block` that is
  no partition and holds a medium, its node in `/dev` opened without
  following a link), opens it read-only with `O_DIRECT` and reads 1 MiB at
  places spread over the disk for two seconds at most. Bursts have two
  places of their own, so disks blinking for minutes never keep the quick
  requests waiting. A sleeping disk wakes.
- Virtual machines (4.13) widen nothing of the system agent's sandbox:
  `virsh` reaches libvirtd over its Unix socket, the domains' XML goes to
  `/etc/zendiloos/vms`, a zvol is made through `/dev/zfs`, `/dev/kvm` and
  `/sys` are only looked at, the bridge a VM goes on is asked of
  systemd-networkd over D-Bus (its own network namespace shows none of the
  host's links), and a console's socket is connected to, which
  needs no write access to `/run`. What writes the volumes (an ISO copied,
  uploaded or removed) runs in transient units that may write only
  `/volume<N>/@vms`; the one receiving an upload has no capability at all
  and reads a pipe whose other end zendiloosd holds, nothing of the
  storage's.
  libvirtd and QEMU run outside, under libvirt's own confinement (AppArmor
  per VM, QEMU as `libvirt-qemu`).
- Backup tasks (4.14) widen nothing of the system agent's sandbox: rsync
  runs in transient units that see of the volumes and drives only the
  roots they work on, write only the target's, reach the network only for
  a server and have only the capabilities their work needs; the NAS's SSH
  key reaches a unit as its credential, a daemon's password on its
  standard input. Runs, restores and retention have four places of their
  own.
- Folder downloads are planned before the first byte (each entry once,
  its name and its folder's index, at most 100 000 entries and 32 MiB of
  names and link targets, both counted while each folder is read), one at a
  time per user and four in all, and given up when nobody reads for five
  minutes; the files agent may use 512 MiB (`MemoryMax`).
- Validated types are the only form in which values reach a command line
  (`DiskName` accepts only `sda`, `nvme0n1` and the like: no paths, no `-`
  prefix).
- Bridges, bonds and VLANs (6) widen nothing of the agent's sandbox:
  their files go to `/etc/systemd/network` as interfaces' files do, named
  only after names the protocol's `InterfaceName` allows (and none of
  Docker's or a `veth`'s), MAC addresses and VLAN ids checked as they are
  read; the agent removes or makes anew only devices of `.netdev` files
  of its own, makes none whose name a link has, and lets an interface be a
  port only of a bridge or bond, and carry only VLANs, of its own files. A
  link is deleted in a transient unit that may do nothing else
  (`CAP_NET_ADMIN`).
- Taking the network over from ifupdown (6), the agent parses nothing: it
  hands ifupdown's files to zendiloosd as they are (only below
  `/etc/network`, never through a link, 256 KiB and 64 files at most) with
  their digest, and takes back interface settings of the protocol's types
  (`InterfaceName`, addresses, prefixes), each interface once, which it
  writes as systemd-networkd files of its own; it refuses them unless the
  files still have that digest. It changes only `/etc/network/interfaces`,
  `/etc/resolv.conf` and those files, after a record of how they were.
- A configuration backup's restore (4.15) writes no text of zendiloosd's
  into a configuration file the agent reads settings from: SSH, the clock,
  TLS and the machine's name go back through their own typed requests, the
  network not at all. What it writes as it is: certificates and keys
  (PEM, checked by `nginx -t` before they stay), Samba's password database
  (as `SetSmbPassword` could set any managed account's), ZFS's host id
  (four bytes) and the IDs handed out (which only rise); users and groups
  only with managed IDs, refused when a name or ID is another's.
- `Secret` has a redacted `Debug`/`Display` and is never logged.
- Every operation is logged to the journal with request ID, peer UID/PID and
  result.
- systemd hardening: `PrivateNetwork`, `ProtectSystem=strict` with explicit
  `ReadWritePaths`, `ProtectHome`, `CapabilityBoundingSet` limited to what
  is needed, `SystemCallFilter`, `NoNewPrivileges` where possible. New
  operations extend this deliberately. The system agent has the
  capabilities `CAP_SYS_RAWIO` and `CAP_SYS_ADMIN` (SMART through SG_IO and
  NVMe admin commands, partition tables, ZFS ioctls), `CAP_CHOWN` (the
  shadow tools) and `CAP_DAC_READ_SEARCH` (the folders apps mount); devices
  closed except SATA/SAS/USB and virtio disks, NVMe (read only) and
  `/dev/zfs`; writable only `/etc`, Samba's and NFS's state directories,
  `/run/issue.d` and `/run/zendiloos-acme`.
- The files agent (`zendiloos-files.service`, the same program with
  `--role files`) serves only requests about share contents. It may write
  below the volumes and act as a user, but touches no disk, no system
  configuration and no network: capabilities `CAP_CHOWN`, `CAP_FOWNER` and
  `CAP_DAC_READ_SEARCH` (share permissions on files owned by users),
  `CAP_SETUID`, `CAP_SETGID` (the file manager) and `CAP_DAC_OVERRIDE`
  (removable drives), which every thread of its runtime drops from its
  effective set as it starts and only a thread working on a drive takes up
  again; `/etc`, `/usr` and `/var` read-only, `/run` too (a drive mounted
  below it later is a mount of its own, writable as mounted); the database, Samba's state, `/etc/zendiloos`, SSH keys and
  the shadow files inaccessible. It does without `RestrictSUIDSGID`: to
  enforce it, systemd blocks `openat2()`, whose arguments seccomp cannot
  inspect, and `openat2()` is what keeps paths inside a share. Volumes and
  shares are created with `setuid=off` and `devices=off` instead, which
  holds for Samba and NFS too.
- `systemd-analyze security` rates `zendiloosd` 1.6, the system agent 2.1
  and the files agent 2.2 ("OK").

### 4.7 Storage Guard Rails

Destructive operations (erasing disks, creating and destroying pools,
replacing disks) go through:

1. **Plan:** the API produces a plan ("these disks will be erased: model,
   serial number, size, what is on them") and a single-use confirmation
   token, valid for ten minutes and only for the account that made it.
2. **Confirm:** the user confirms; creating and destroying a pool also need
   its name typed again. Right before carrying it out, `zendiloosd` makes
   the plan again from the disks as they are now and compares disks by
   `/dev/disk/by-id` link, serial number, size and contents: if anything
   differs, for example because another disk now has the same kernel name,
   nothing happens (`plan_changed`).
3. **The agent checks again, independently of `zendiloosd`,** before it
   erases a disk: the by-id link leads to a whole disk that is not
   read-only; neither the disk nor a partition is held by another device
   (md, LVM, dm-crypt), mounted (`/proc/self/mountinfo`) or used as swap;
   the disk and its partitions can be opened exclusively (`O_EXCL`), which
   fails while ZFS, md or a filesystem uses them. Pools are destroyed or
   changed only if their `zendiloos:volume` property shows that zendiloos
   created them. Disks travel as by-id names, never as kernel names.
   A pool is destroyed only when nothing else lives on it: shares, apps,
   the personal folders and Docker's data go first, each through its own
   place (`pool_in_use` names them; the agent refuses on its own while
   Docker's `data-root` is there). Every mount of the pool is stopped
   before anything is removed; if one is busy, the stopped ones are mounted
   again and nothing changes, and their units are removed only once
   `zpool destroy` succeeded.
4. **Configuration:** `[storage] erasable` in the agent configuration is
   `"unused"` (every disk that passes the checks; the default), `"none"`,
   or a list of by-id names, which restricts a machine to the listed disks.
5. **One at a time:** storage jobs run one after another, and the agent
   serializes changes to disks as well.
6. **Removable drives** are only mounted and powered off, never erased,
   and only what is plainly one: a FAT, exFAT, NTFS or ext4 file system
   with a UUID, the only one with it (a cloned stick is left alone),
   directly on a USB or removable disk or on a partition of one, nothing
   held on top of it, on a disk where nothing is a ZFS, RAID, LVM, bcache
   or LUKS member, an array or a mapping, and nothing is mounted or used
   as swap except the drive itself at its own place, so neither a system
   booted from USB nor a pool on a USB disk qualifies. `zendiloosd` offers
   only such drives, and the agent checks again (from `lsblk`, with its
   own parser) before it mounts one or powers a disk off; the power-off
   itself checks once more that the disk hangs on USB or is removable.

Destructive features are developed and tested only against `zendiloos-sim`,
fake hosts in tests (devices, sysfs and mountinfo in a temporary
directory) and virtual disks in the Debian VM (`just vm-storage-test`).

### 4.8 Audit and Log Hygiene

- Audit table (append-only from the API's point of view): time, user, source
  IP, action, target, outcome, request ID. Includes logins and failed
  attempts. Every context writes it through the shared `AuditLog` port of
  the kernel. Actions that may end the process (reboot, power off) are
  recorded before they are carried out, and a failure is recorded after.
  While background jobs run, or an update is being installed (its release
  coming, apt running), they are refused (`jobs_running`, recorded as
  denied): a copy, a share's permissions or a pool's change would stop
  halfway, dpkg cut off would leave packages half done, and the agents would
  hold the shutdown until they saw it through. The system agent refuses
  them itself while `zendiloos-packages.service` runs.
  The request ID also appears in the agent's journal.
- Structured logs through `tracing` (journald in production, readable during
  development). Passwords, tokens and cookies never appear in logs; the types
  ensure this, not discipline. When writing to the journal, each line
  carries its syslog priority (`<4>` for warnings and so on), so the log
  viewer can filter by priority.

### 4.9 Apps (Docker)

Access to the Docker socket is equivalent to root. `zendiloosd` never
reaches it; the agent does, and only to run what an administrator
confirmed.

**The App Center is a way into Docker.** An app of a catalog is the
template of an administrator's own Compose project: the App Center fills
in its Compose file with what the administrator gives and makes it a
project in a folder they pick, which is theirs from then on, changed,
updated and removed in the Docker app like any other (see *Installing*
below). Apps installed before it worked so keep running as they were.

**Catalogs.** Apps come from catalogs in the format of the
[community catalog](https://github.com/mavnezz/zendiloOS.community.applications)
(`docs/format.md` there): per app a manifest (`app.yaml`), a Compose file
and an icon, published as one `index.json`. The community catalog
(`https://mavnezz.github.io/zendiloOS.community.applications/v1/index.json`)
is built in and cannot be removed. Administrators add more: a GitHub
repository (`github.com/<owner>/<repo>`, read as
`https://<owner>.github.io/<repo>/v1/index.json`) or the https address of
an index in the same format. Adding one is confirmed after a warning, and
its apps say they are not from the zendiloOS community catalog. `zendiloosd` fetches
indexes over https (redirects only to https, a size limit, a timeout) and
checks every entry with the catalog's own checks before showing it; an
entry that fails is left out, not repaired. Icons are cached, checked to be
PNG and served as `image/png`. Each app names the architectures its
images are made for (`architectures`, which the catalog's checks hold
against the registries): an app whose list lacks the NAS's is shown, marked
(`runs_here` false), and its template is not filled in (`app_architecture`,
409), so a project never waits for an image Docker cannot pull.

**One policy, in one crate.** The format and the policy live in the
catalog's crate `zendiloos-catalog` (pure: text in, problems out).
`zendiloos-infra` uses it, to check the indexes, as a git dependency
pinned to one commit, so a change to the catalog's checks reaches zendiloOS only when
the pin is moved deliberately: Renovate opens a pull request for each new
commit of the catalog (`renovate.json`), CI checks it, a maintainer merges
it, the next release ships it. The apps themselves are read live from the
index, so new apps need no release. Within `format: 1` the catalog stays
readable by released versions of zendiloOS; what they cannot read gets a
new index (`/v2/`) beside the old one. The policy decides which apps a
catalog offers: images following a tag of their developers, bind mounts
only from the app's data folder and the shares the administrator picks,
published ports from variables, no Docker volumes, no `privileged`, no
host namespaces; devices of the kinds the format lists (graphics, VPN,
serial, cameras, sound, Coral TPUs), extra capabilities (`NET_ADMIN`,
`SYS_NICE`), the host network and the Docker socket only as permissions
the app declares. Reading an index checks the manifest and the Compose
file as written; the policy proper, on what Compose makes of the file, is
checked when its template is filled in (see *Installing*). An app made
into a project is then the administrator's
file: what it lets its containers do beyond themselves (those
permissions among it) is confirmed like any project's, see *What runs is
what was confirmed*. Generated secrets take the length and alphabet the
catalog asks for (32 letters and digits otherwise).

**The Docker app** (context `zendiloos-containers`) shows the engine as
it is: every container, image, network and Compose project, whoever made
them; containers and projects made elsewhere are started, stopped,
restarted, updated and removed here, an app installed the earlier way
only started, stopped and restarted (the App Center removes it, and its
Compose file holds generated settings, so the agent shows it to no one).
A project made from an app of a catalog is an administrator's own and
names the app it came from (`template`). The agent
runs `docker` with fixed arguments and checked names (`ContainerId`,
`ImageReference`, `ProjectName`, …) and hands back what it printed;
zendiloosd reads it. Container logs are both streams in the order they
were written.

**A container's page** (its name opens it, in the list or in a project)
shows it as `docker container inspect` tells it (`InspectContainer`):
command, user, environment (values hidden until shown: they hold
passwords), networks with its addresses, published ports, mounts, what
reaches beyond it (privileged, capabilities added and dropped, devices,
security options, a read-only root, the machine's processes or shared
memory), limits, restart policy and health check, and its use now and
over time. A bind of a project made here is a pin Docker sees; the page
names the folder the confirmed file names for that target instead
(`/volume1/media/Films`, shown as the share's folder `media/Films`). Its
processes come from `docker top` with fixed columns
(`ContainerProcesses`). Its log is asked again every three seconds while
it is followed, searched in the browser and saved as a file.

**A terminal in a container** runs what the administrator types, split
at spaces into at most 16 words, the first no option of `docker exec`
(`ExecCommand`). The agent starts `docker exec --interactive --tty` on a
pseudo terminal of its own (`OpenTerminal`), as the leader of its own
session through `setsid --ctty` (no code may run between fork and exec
here), with Ctrl-P Ctrl-Q no longer leaving the program running unseen,
and hands the terminal's master to zendiloosd with the answer
(`SCM_RIGHTS`); it waits for the program itself and ends it after a day at
the latest; sixteen at once. zendiloosd relays the master to a WebSocket,
`/api/v1/docker/containers/{id}/terminal`, of the same origin and for
`containers.manage` only: binary frames both ways, a new size as text,
set on the terminal (the program gets `SIGWINCH`). Closing the socket
closes the master, which hangs the terminal up and so ends the program
(one that ignores the hang-up may outlive it in the container, as with
`docker exec` anywhere). A terminal ends when its program does, after half
an hour without input, or when the session behind it ends or loses the
permission (checked every minute); the socket is pinged every 30 seconds
so nginx keeps it. Four per account; each is recorded in the audit log
(`containers.container_terminal`, with the command). Any running container
gets one, an app installed the earlier way too: a terminal changes nothing
of how a container was made, and administrators are as good as root here
anyway.

**Administrators' own projects** have no policy: an administrator may run
any Compose file, which makes an administrator's account as good as root
on the machine. The file lives in a share (`compose.yaml` in a folder such
as `docker/teslamate`), written and read through the file manager's use
cases with the administrator's permissions, so it is backed up and
snapshotted with the share; relative paths start there. Before a file is
deployed, `docker compose config` reads it (from stdin, in that folder; a
`.env` there does not count), and what reaches beyond the containers is
named: `privileged`, the Docker socket, folders outside the volumes and
those zendiloOS keeps on a volume for itself (`@docker`, Docker's data,
where the containers' settings are root at their next start; `@homes`;
`@apps`; anything else starting with `@` or `.`, as no share's name
does), also as named volumes that bind them (`o: bind` or `rbind`), the
host's network (also joined as a network named `host`), PID, IPC or user
namespace, devices (also a disk's partition a named volume mounts with
`type` and `device`), extra capabilities, no seccomp or AppArmor,
settings not known as harmless (named volumes of another driver, of
another file system than memory or another machine's share, or made
outside the project; networks that are not a bridge of the project's own:
made outside it, of another driver, with driver options), and what takes
in files whoever may write the share can change (`build`, built from the
share at every deployment; `env_file`, `extends`, `include`, read when it
is saved). Each must be confirmed, then and on every change;
the audit log records `risks_accepted`. A confirmation holds for what was
checked and nothing else: the check (`POST /docker/compose/check`) names
it by a digest, SHA-256 of the file and of what Compose made of it with
all it took in, the confirmation gives that back (`accepted`), and the
project is made or changed only if Compose makes the same of the file
then; otherwise it is refused (`compose_changed`) and checked again.
Whoever may write the share could change an included file, an `env_file`
or an extended one between the check and the save, and what was shown
would no longer be what runs. The folders inside the project's
folder its binds mount (`./config`) are made, as far as they are missing,
through the file manager when the file is saved, as the administrator's
own (the one exception to the rule that what is made in a share takes
its folder's owner, 4.4): Docker would make them as root, and a container
run as the administrator could not own them (a database refuses a data
folder of another owner). Deploying (pull, then
`up --detach --remove-orphans`) and removing (`down`; images, data and the
folder stay) are jobs. A redeploy pulls only when asked ("pull the newest
images"; containers no longer in the file are removed either way).

**What Compose writes while it deploys** is shown as it comes: the agent
runs it (`pull` and `up` without `--quiet`) reading both streams line by
line (`zendiloos_exec::run_lines`: a `\n` or a progress bar's `\r` ends a
line, a line longer than 4 KiB comes in pieces) and sends each line to
zendiloosd as a `Response::Output` before its answer, as pool steps come
as `Response::Progress`. The job keeps them as its `output`: the last 200
lines, each at most 300 characters, terminal escapes and other control
characters taken out (`zendiloos_jobs::clean_line`); it is stored with the
job and published on the `jobs` topic at most every quarter of a second
while lines come, saves of one job one after another so its end is always
what was published last. The browser shows the lines as text in the
deploy dialog (creating, changing and redeploying a project), never as
markup; an image update's job keeps them too.

**A project's page** has its state, when and by whom it was made, where
its Compose file is in the shares (`docker/teslamate/compose.yaml`),
start, stop and restart, and tabs: its containers (state, image and tag,
CPU, memory and network now; each opens the container's page, the window
section `container:<name>`), resource monitoring (CPU, memory, upload and
download over time, one line per container, from the samples below), the
log of all its containers together (`docker compose logs`, each line
under its container's name in a colour of its own, followed while shown,
searchable, downloadable as text) and its Compose file.

**What runs is what was confirmed**, never the file in the share: whoever
may write that share (other accounts, an NFS client, an app given it)
could change the file, a `.env` or an included file between confirming
and the next deploy, redeploy or scheduled image update, and Compose runs
as root. So zendiloosd keeps the file as saved (the editor shows it) and
what Compose made of it then, normalized as JSON with includes and
`env_file` resolved, in its database, and deploys that (`DeployProject`).
The copy in the share is for looking at. The agent pins every bind whose
source lies in a volume, as for apps (the folder or file walked to without
links, mounted under `/run/zendiloos-project-shares/<project>/`, a unit per
project before Docker at boot; a missing folder a bind may create is made
there, as Docker would; each pin is named by a hash of what it pins, so a
bind whose source changed gets another and its container is made again),
named volumes that bind (`o: bind` or `rbind`), secrets and configs from
files alike, and no bind into a volume lets Docker make a missing folder
(a pool not mounted yet would get it on the system's disk). A source that
is relative or goes through `..` is refused: the kernel would not take it
as it reads (a relative device starts where Docker runs, at `/`; Compose
makes it absolute only for `o: bind` itself), and it could reach a share
unpinned. It keeps the result as
`/etc/zendiloos/projects/<project>.json` and runs Compose on that file with
an empty `.env`; `down` removes it again. A deploy changes nothing of
what runs before its images are there: it pulls from a file of its own
(`<project>.json.pull`, removed afterwards), then pins the new binds
beside the old ones, keeps the file and brings the project up, then asks
Docker what the project's containers and volumes mount and leaves pinned
only that (and, once it came up, the new binds), after a failure too, so
neither a container Docker restarts loses its folder nor do pins of a
failed deploy linger; while Docker cannot say, every pin stays. A project made before this was
kept runs once its file is saved again. A project made elsewhere whose
Compose file lies in a share is not deployed here, for the same reason,
nor one that binds a folder or file in a share (below the share's root,
also as a named volume, a secret or a config): it runs from its own
files, by those paths, so Docker would follow a link someone put in a
folder's place. Before `up` or `pull` of such a project (a redeploy, an
image update) the agent reads what Compose makes of its files and
refuses; it is made a project of zendiloOS's (its file saved here, which
pins what it binds) or binds the share as a whole. Its containers are
still started, stopped and restarted: that changes nothing of how they
were made, and Docker restarts them by itself anyway. Asking for a
confirmation instead would not make them run pinned, and a scheduled
update has nobody to ask.
A share a project lives in or mounts something of is not deleted
(`share_used_by_projects`; zendiloosd from the confirmed configurations,
the agent again from the kept files, which then offers the share again).

**Use over time:** zendiloosd samples `docker stats` every 20 seconds and
keeps twelve hours in memory, by container name, with the network's rates
from its counters.

**Newer images:** every six hours (if wanted) zendiloosd asks the
registries for the digest under each tag the containers run, with a
`HEAD` of the manifest (anonymous tokens where a registry wants one; no
pull counted), and compares it with the image's digests. On a schedule
(days of the week and a time, the machine's local time) or by hand,
projects with a newer image are pulled and brought up again. The schedule
is due as a backup plan is, with the same calendar (daylight saving time
included): once its latest time lies after it was set or last ran
(`docker_update_settings.schedule_since`), so a minute missed (the NAS
off, asleep or busy, its clock changed) is made up once at the next
look, a clock that was ahead counts from when it is right, and one set
before that was kept counts from the first look; containers
outside a project are only pulled for, apps of catalogs left to the App
Center, images pinned by digest never looked at. A container with a newer
image says so in the list, which also says how many can be updated; one
is updated by itself the same way, in a job (its project pulled and
brought up again, which makes its containers anew; outside a project only
its image fetched; an app installed the earlier way refused). The
interface asks first, saying how it goes for that container and that it
stops meanwhile and loses what is not in a mounted folder or volume,
unless the browser was told not to ask again.

**Volumes** are listed as `docker volume inspect` tells them
(`DockerVolumes`): name, driver, the Compose project that made them (its
label), when they were made, and the containers that mount them, running
or stopped, from the containers' mounts (each opens the container's page).
A volume removed between the listing and the inspection is left out (each
is then inspected alone). What their data takes up is asked apart
(`DockerVolumeSizes`), so the list does not wait for it: the agent counts
each local volume whose data lies in Docker's data root
(`<DockerRootDir>/volumes/<name>/_data`, as `docker info` and the volume's
mount point agree) by descriptor, never following a link or entering
another file system, a file with several names once, at most a million
entries a volume and twenty seconds in all; a count cut short, or one that
could not look into a folder, says "at least". It holds a descriptor or
two however wide or deep the folders are: it goes down by name and back up
by `..`, which must be the folder it came from, and keeps only the names
still to count. What it remembers is bounded, not the volume: the folders
still to count up to 64 MiB (each reckoned at its name and 64 bytes
beside), and up to a million files with several names (under 40 bytes
each), to count each once; a count out of room counts less and says "at
least", never more. Counting has a place of its own in the agent, one at a
time. `docker system df -v` would walk every volume without bounds, and
cannot print JSON. A volume no container uses is removed (`docker volume
rm`, without `--force`; its backup plan goes with it, its backups stay,
on the Backups page),
and all unused ones at once (`docker volume prune --all --force`, named
ones too), each after a confirmation; pruning is refused while a volume is
backed up or restored, or waits for its turn.

**Backups of volumes:** one plan per volume (`docker_backup_plans`): a
folder of a share the administrator may write, chosen with the App
Center's folder picker; daily, weekly on a weekday or monthly on a day (the
month's last where it has fewer) from a local hour of the host, as the
SMART self-tests are planned; how many backups are kept (seven unless
told otherwise); and whether the containers that use the volume are
stopped meanwhile (yes unless told otherwise: a database's files are then
consistent). A new plan starts at the night's hour (one to five) the other
plans take least, so that they spread over the night. The dialog says that
whoever may read the chosen share reads all of the volume's contents in its
backups, also what the containers keep to themselves (database files, keys,
`.env` files), and names who that is (`GET
/docker/backup-shares/{id}/readers`: the accounts and groups its rules let
read, the NFS clients it is exported to). Each date's hour is a moment with
the time zone's offset of that date (where the clock shows the hour twice,
as summer time ends, the first time; where it skips it, the hour after), so
a plan runs once a day across the change. A run is due once the latest time
of its schedule lies after the plan was made, changed or last ran: a NAS
that was off at that hour backs up once as soon as it is on again
(zendiloosd looks every minute). A plan noted later than now (the clock was
ahead and is put right) counts from now on. Only local volumes without
mount options (an NFS or bind volume's data is not in Docker's folder) are
backed up.

A plan whose volume is gone when a run is due (removed outside zendiloOS,
e.g. by `docker compose down -v`) is paused (`paused`): no run, no failure
told night after night, a line in the log; its recorded backups stay until
they are removed. "Back up now" is refused while it is paused
(`plan_paused`). A volume of the same name made again does not resume it:
the administrator does (`POST /docker/volumes/{name}/backup-plan/resume`,
refused while there is no such volume), or sets the plan anew, or deletes
it. Resuming by itself would be the wrong guess as often as the right one:
the new volume may be another app's, or empty, and the plan's retention
counts its backups and the old ones alike, so a few nights of the new
volume's backups would remove the last ones of the volume that was
deleted. Runs are due from the resumption on.

A run is a job, by hand ("Back up now") or on the schedule, and so is a
restore. Each claims its volume when it is asked for (one at a time per
volume, `volume_busy` otherwise) with the containers it will keep stopped:
every container that uses the volume, if it stops them, whatever Docker
said of it a moment before (running, being stopped, stopped by another
backup or restore, being started again by one that ended; one stopped by
hand is owed no start, so holding it starts nothing). Backups and restores
then take turns, one at a time on the whole NAS ("waiting" meanwhile; a
claim holding a container another one stopped goes first), and only once
its turn came, right before
the agent starts the work, does a run stop the running containers that use
the volume. Each is noted first as owed a start (`docker_owed_starts`, with
the volume). A container stays stopped until the last claim holding it
ended: a container of two volumes backed up at the same hour is stopped
from the first's start to the second's end, and started once. Meanwhile a
held container is not started, stopped, restarted or removed, nor its
project controlled or deleted (`containers_held`); deploys, redeploys,
image updates and updates of one container wait in their job until nothing
holds the project's containers; Docker's data does not move while a volume
is claimed, and no volume is claimed while it moves. When a run ends,
whatever came of it, the containers nothing else holds are started again,
each looked at right before its start (one a backup or restore asked for
meanwhile holds stays stopped for it), and only then does its claim go: it
keeps its turn while they start, so no other backup or restore begins
beside a start and finds a container stopped that is about to run. Each is
forgotten only once its start succeeded. A container stays owed when its
start fails, and every minute, and when zendiloosd starts, those nothing
holds are started (not while a backup or restore has its turn), each only
after the agent said nothing works on their volumes any more
(`EndVolumeWork`, which stops what a restart left). zendiloosd is not
ordered after
`docker.service`: Docker waits for its pool's mount and has no start
timeout, and the web interface must come up without it; the minute's look
starts the containers once Docker is up. The run is recorded
(`docker_backups`) with what it wrote, and a run that failed or could not
start a container again is told (`docker_backup_failed`, in the routing
group *Docker*). The audit log has `containers.backup_plan`,
`containers.backup_plan_delete`, `containers.backup_plan_resume`,
`containers.volume_backup`, `containers.volume_backup_remove` (by
retention and by hand), `containers.volume_backup_import` and
`containers.volume_restore`.

**The Backups page** (`GET /docker/backups`) lists every volume with
recorded backups or a plan, by name, also those gone (marked deleted), with
its plan (paused or not) and its backups: size, time, result, who started
it, whether it was imported. From there a backup is restored, also one of a
volume deleted (under its own name, which makes it again, or into a volume
that exists), and removed by hand (`DELETE /docker/backups/{id}`: archive
and manifest, each while it is still the file written, and the record; of a
run that failed, the record only), not while it runs nor while any backup
or restore is under way, as one may be reading it.

**Importing backups** (`POST /docker/backups/import`, a job,
`containers.volume_import`) records the backups in a folder of a share the
administrator may write and in the folders in it, for a database that lost
them (zendiloOS set up anew, its database restored from before). The agent
looks (`ScanVolumeBackups`, in a transient unit of its own with
`CAP_DAC_READ_SEARCH` only and the system read only, at most 2000 folders
and 2000 archives) at every `<volume>-YYYYMMDD-HHMMSS.tar.zst`, by
descriptor beneath the share without following a link, and takes it only if
the archive and the manifest beside it are regular files of root's that
nobody else may write (no group or other write bit, so no ACL entry may
either: the group's bits are the ACL's mask), the manifest names this
archive and volume, and the archive's SHA-256, hashed as it is read, is the
one the manifest names. What it refuses is listed on the job with why
("the archive: writable by others", "no manifest beside it", "its SHA-256
is not the one its manifest names", …); archives zendiloosd recorded
already are passed over, before and after the agent looked. An imported
backup is recorded as a run that succeeded, at the time its manifest
names, of no plan (`imported`, no one started it): retention never removes
it, and it does not count toward any plan's number; it goes when it is
removed by hand. Its restore checks it as any other: by the SHA-256, data
and entries recorded, with the driver and labels of its manifest as read
at import. The agent's answers come in pieces of 1500 bytes
(`{"part": …}`), as a unit's lines are cut at 4096, so neither a long list
of findings nor a volume with long labels is cut short.

The agent checks a backup itself (`BackUpVolume`): a local volume whose
data lies in Docker's data root, as it inspects it, a share that is
mounted, and no unit of the volume still running. The work runs in a
transient unit (`systemd-run`, this binary as `backup-volume`, the job as
hex digits on its command line), outside the agent's sandbox, which can
write neither shares nor Docker's data, in a sandbox of its own: no
network, no devices, no other processes seen (`ProtectProc=invisible`), its
own IPC, `SystemCallFilter=@system-service` (not `RestrictSUIDSGID`:
systemd enforces it by blocking `openat2()`, which keeps the units' paths
beneath their folders), the whole system read only but the share,
`CAP_DAC_READ_SEARCH` and `CAP_DAC_OVERRIDE` only, 65536 descriptors and
1 GiB of memory (`MemoryMax`). Each
unit has a name of its own,
`zendiloos-volume-<backup|restore|swap>-<volume>`, so systemd refuses a
second one, and may run until ten minutes before the agent stops waiting
(`RuntimeMaxSec`; six hours in all, zendiloosd waits ten minutes longer).
Should the client hang up, the agent be asked to stop or the time run out,
the agent stops the unit and waits for it before it answers: no unit works
on behind an answer. Backups, restores and their removals have two places
of their own in the agent, beside the long ones of deploys, pulls, drives
and pools. When the agent starts, it stops the volumes' units still running
(their client is gone) and has a unit remove what restores left beside
every volume's data (`sweep-volumes`); backups and restores wait for it.

The backup reads the volume's data as root, by descriptor, never following
a link or leaving its file system, one name of a folder at a time, and
writes one archive per run, `<folder>/<volume>/<volume>-YYYYMMDD-HHMMSS.tar.zst`
(the host's local time): tar with GNU headers (long names, numeric owners
and groups, modes with set-ID bits, times, symbolic links as links, hard
links as links to the first name the archive holds, extended attributes
`user.*`, `security.capability` and the POSIX ACLs as `SCHILY.xattr`
records), compressed with zstd, readable by Debian's tar and zstd too;
sockets, pipes and devices are left out, and folders nested deeper than
1024 fail the backup. However many entries a volume holds, the backup
takes no more memory for them than the folders it is in and a bounded
table of first names: those of a million files with several names at
most, or 256 MiB of them (each reckoned at its path and 128 bytes beside);
a later name of a file beyond is written whole, so its data is in the
archive all the same. Its progress comes from counting the volume first,
in memory bounded as above. Beside it goes
`<volume>-YYYYMMDD-HHMMSS.json`, its manifest: the volume's name, driver,
labels, size and when it was made, the release's version and the
archive's SHA-256. The folders on the way are walked to beneath the share
(`RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV`) and made as far
as they are missing (0777, which the share's inherited default ACL
narrows), and hidden files earlier backups left in the volume's folder
there (a unit stopped while it wrote; regular files of root's named as
backups name them) are removed first. Both files are written completely
under hidden names (`O_CREAT | O_EXCL | O_NOFOLLOW`, mode 0666, so the
share's inherited ACL decides who may read them), made writable by root
only (the group's and others' write permission goes, and with the group's
the ACL mask's, so no entry of the ACL may write them; who may read and
remove them is as the folder's ACL says), and only then renamed into place
(`RENAME_NOREPLACE`), the archive first; should the manifest not follow,
the archive is removed again by its identity, so no archive is left
without its manifest. Their paths in the share are formed before anything
is written, and the manifest is complete before either is renamed: once
both are in place, the unit does not fail any more and leave them behind
unrecorded. zendiloosd records each file with its size, identity
(inode, and the time it was written) and device, the archive's SHA-256,
the data's size, its entries and the manifest. Retention removes the
archives of the runs beyond the plan's number, the newest as recorded kept
(never by their clock: the archive just written always stays), and only
those recorded: the agent removes a file only while the name still is that
file (`RemoveVolumeBackup`: its inode, size and time of writing, on the
share's file system as it is mounted now; a ZFS dataset gets another
device number each time it is mounted), never by a pattern; a file gone or
replaced is left alone and forgotten. Failed runs are remembered ten at a
time, after every run.

**Restoring a backup** (`RestoreVolume`), from the volume or the list of
its backups, goes into the volume of its own name (made again if it is
gone: local, with the driver and labels of its manifest as the database
keeps it, never as the copy in the share says, which others may change;
removed again if the restore fails) or into another volume that exists,
after a confirmation that names it; running containers that use it are
stopped meanwhile and started again after only when asked to, otherwise it
is refused (`volume_in_use`), also when its turn came. The archive lies in a
share that users may write, and restoring runs as root; so only an archive
zendiloosd recorded is restored, by its recorded SHA-256, data and entries.
The agent refuses while a running container uses the volume or a unit of
the volume still runs, and runs the work in two transient units (the
system read only but the volume's folder in Docker's data root;
`CAP_CHOWN`, `CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`, `CAP_FOWNER`,
`CAP_FSETID`, `CAP_SETFCAP`; set-ID bits allowed, as modes come back). The
first (`restore-volume`) removes what earlier restores of the volume left
beside its data (fresh folders of a unit stopped or killed by its time or
memory), refuses when the volume's file system has less room free than the
backup's data, opens the archive beneath the share without following a
link (it must be a regular file, and writable by root only: one others may
write could change between its check and its extraction), checks its
SHA-256 before anything is extracted, then extracts it into a fresh folder
beside the data (`_data.zendiloos-restore-<random>`, 0700) while the same
bytes are hashed again, no more than the backup's data and entries and a
little (an archive changed after its check never fills the pool): every
entry's path is names beneath that folder (an absolute path, `..` or an
odd name refuses the archive), every folder on the way is opened with
`openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS |
RESOLVE_NO_XDEV)`, files are made with `O_EXCL | O_NOFOLLOW`, hard links
name entries beneath without following a link, devices and other kinds
refuse the archive; owners, modes, times and the extended attributes above
are set on each through its descriptor, folders once the entries left
them, so only the folders an entry is in are kept in memory, however many
entries the archive holds. Only if the
second hash matches too and the file was read to its end does the agent
look for running containers once more; if one uses the volume now, what was
extracted goes and the old contents stay (`volume_in_use`). Otherwise the
second unit (`swap-volume`) swaps the fresh folder in for the data in one
rename (`RENAME_EXCHANGE`) and removes the old contents; what it could not
remove is named on the job and goes with the next restore of the volume or
the agent's next start. Removing a tree holds a descriptor or two however
deep it goes. Any failure removes the fresh folder and leaves the old
contents as they were.

**Moving Docker's data** to another pool zendiloOS made: Docker stops, the
dataset is copied with `zfs send -R | zfs receive -u` (up to six hours,
over a transport that waits as long), the mount unit, the drop-in and
`data-root` follow, Docker starts from the copy, and only then is the old
dataset destroyed; if the copy fails, or switching to the copy fails,
Docker goes back to its data as it was and starts again, never left
stopped. It is refused while a volume is backed up or restored, or waits for
its turn, and none is while it moves. While it copies, other changes to pools go on (the pools' lock is
held only to check and to switch; one move at a time); the old pool cannot
be deleted meanwhile, Docker's data root still being there. Containers that
restart by themselves start again. The apps' datasets stay where they are.

**Docker's own data** (images, layers, containers) lives on a pool, not on
the system disk: setting up the App Center's Docker plugin creates the dataset
`<pool>/zendiloos.docker` (the dot keeps it from ever being a share),
mounted by its own unit at `/volume<N>/@docker`; a drop-in
(`RequiresMountsFor`) keeps Docker from starting without it, so it never
writes into the empty folder beneath; and the agent sets `data-root` and
the `overlay2` driver in `/etc/docker/daemon.json`, keeping whatever else is
there. `overlay2` runs on OpenZFS 2.3 with native overlay diffs, without
the dataset per image layer of Docker's `zfs` driver. Docker comes from
Debian (`docker.io`, `docker-compose`, suggested by the package); its
data from before stays where it was, unused. Verified in the second test
VM, across a restart.

**Installing** an app of a catalog makes it an administrator's project,
in three steps of the install dialog:

1. *Where*: a share the administrator may write to (the Docker app's
   places), a base folder in it (`docker/<app>` unless changed) and the
   project's name (the app's id unless changed; one Compose allows and no
   other project has).
2. *What it gets*: ports (checked against zendiloOS's own, those of apps
   installed the earlier way and those something listens on now,
   `/proc/net/{tcp,udp}*`), the shares or folders in them it sees, picked
   from those the administrator may use as the app does (only writable
   ones where it writes, since it runs as them), and its settings;
   generated ones are made then. `POST /apps/catalog/{source}/{app}/compose`
   fills the template in: `APP_DATA` is the project's folder, written
   relative (`./config`); `SHARE_<ID>` the picked folder's path on the
   machine; `PUID` the administrator's UID, `PGID` the GID of
   `zendiloos-files` (the group whose access every share's folder gives,
   4.4, so the containers read and write the share folders they mount
   through the share's one ACL) and `TZ` the time zone, both from the
   system agent (`HostFacts`, which also says
   which of the app's devices the machine has; the lines of the others are
   commented out, so the app still runs without a GPU or a Zigbee stick).
   Every value is filled in where Compose would interpolate it, in the
   values of the YAML read with their places in the text, quoted so that
   it reads back as exactly that value and with `$` doubled; comments and
   the rest of the file stay as written, and the result is read again and
   must hold the same document with the values in. The request names the
   project too (name, share and folder, as in step 1), for the catalog's
   policy: what the index's checks see is the file as written, and much
   the policy forbids shows only once Compose has made one meaning of it
   (a named volume that binds `/`, `${APP_DATA}/../..`, `privileged`). So
   the template is filled in the same way with the catalog's stand-in
   values (`Placement::stand_in`, as the catalog's validator fills it),
   read by `docker compose config` in the project's folder
   (`CheckCompose`), and checked with the catalog's `check_normalized` and
   `check_variables`; an app that breaks the policy is refused
   (`against_catalog_policy`, naming what), whatever its manifest says of
   its permissions, which are all the App Center shows. The
   administrator's values then take the stand-ins' places, so the file
   they get differs from the one checked in those values alone.
3. *The file*: shown in an editor, to read and change, then made a project
   with `POST /docker/projects` naming the app as its `template`: checked
   as Compose reads it, what reaches beyond the containers confirmed,
   written to the folder, its bind folders made by the administrator, and
   deployed. Generated secrets stand in the file as any password an
   administrator writes into a Compose file: whoever may read the share
   reads them.

From then on the App Center only finds the project again by its template
(`GET /apps/projects`): it shows the app as installed, opens its web
interface where the containers publish the port the template put the
app's web port on (a fixed port where it always is), and leads to the
project in the Docker app. A newer version of the app in its catalog
changes nothing: the images are updated with the Docker app's image
updates, the file by the administrator.

**Shares and apps.** A container sees only the folders its Compose file
mounts, which the administrator confirmed (above); what it may do there
is a matter of file access alone, as for anything on the machine: new
installs run in the group `zendiloos-files`, and the shares' one ACL does
the rest. A share's rules (4.4) are the people's, for Samba and the file
manager; they do not reach into containers.

**Apps installed the earlier way** each got a dataset of their own
(`<pool>/zendiloos.apps/<id>` at `/volume<N>/@apps/<id>`), an account
`app-<id>` with access to the shares picked, and a Compose file the agent
checked against the
policy and keeps, normalized, readable by root only
(`/etc/zendiloos/apps/<id>.json`). The access they were given stays in
their entries on the shares' files, which the update to share-level
access leaves alone (4.4), so they keep working; the Sharing context keeps
the record of it (`share_app_access`) as information only: a share an
app uses is not deleted (`share_used_by_apps`), and the record goes with
the app. They keep running: they are started
from that file (the agent walks each folder they mount again first),
stopped, their logs read, and removed; nothing installs, tries again or
updates an app so any more, and the agent has no steps left for it (no
preparing, checking, pulling or making folders). One whose installation
did not finish is *failed*, to be removed. Removing takes
the containers, images and kept file, destroys the dataset with the data,
then forgets the account's share access and finally takes the account.

**Folders in shares are pinned**, for the apps installed the earlier way
as for administrators' projects. A share's root is a dataset's mount
point, which only root can move; a folder in it can be swapped for a link
by anyone who may write to the share (an NFS client, another app), and
Docker follows links in a bind mount's source every time it starts a
container, also when it restarts one by itself. (Debian's Compose 2.26
ignores `volume.subpath` without a word, so Docker's own safe sub-paths
are no way out.) So the agent never hands Docker such a folder by its
path: it walks to it from the share's root with `O_PATH | O_NOFOLLOW`,
staying on the share's file system, and mounts the very directory it
opened (`open_tree` and `move_mount` on the file descriptor) at
`/run/zendiloos-app-shares/<app>/<share id>`, a folder only root may
change; the kept file binds that place, and the bind refuses to start rather
than let Docker make an empty folder when the pin is missing. The mounts
belong in the host's mount namespace, outside the agent's sandbox: a unit
per app, `zendiloos-app-<app>-shares.service` (`RequiresMountsFor=` the
shares, `Before=docker.service`, no namespace of its own, only
`CAP_SYS_ADMIN` and `CAP_DAC_READ_SEARCH`), runs the agent's binary as
`pin-shares <app>` at boot and before every start, and unmounts when
stopped; it goes with the app. Before a start the agent also checks that
the pin is the folder the walk reaches now (same device and inode). A
folder swapped for a link keeps the running app on the folder it had and
makes the next start fail with the folder named. Verified in the second
test VM, across a reboot, while apps were still installed this way.

### 4.10 Updates and Debian's packages

On an existing Debian the NAS keeps itself current like OPNsense does:
whatever needs no restart first, the kernel last, a restart only when the
administrator asks. One update is one run: apt's lists, Debian's upgrades
(`apt-get --with-new-pkgs upgrade`, keeping changed configuration files,
never removing a package), then zendiloOS from a release, then a look at
the kernel modules.

**apt never runs in the agent's sandbox.** It writes all over the system
and fetches over the network, and the agent has neither (`ProtectSystem`,
`PrivateNetwork`). The agent starts a transient unit,
`zendiloos-packages.service` (one at a time), which runs the agent's own
binary as `run-packages <id> <task>` outside the sandbox; the runner records
each step in `/var/lib/zendiloos-packages/run.json`, which the agent reads
back. A run outlives a restart of the agent and of zendiloosd, which matters
when it installs zendiloOS: the package's maintainer scripts restart both,
and the page follows the run from its record once zendiloosd is back. A
tab left open across an update still runs the old build, whose lazily
loaded parts are gone: when loading one fails (`vite:preloadError`), the
page reloads once for that build (noted in `sessionStorage` with the
entry script's URL, which names the build), and should the same build
fail again within five minutes, a note asks to reload by hand instead of
reloading on and on.

**An update and long work exclude each other**, by refusal, never by
waiting in a queue: an update (and finishing one) does not start while
background jobs run (`jobs_running`, the detail names them, recorded as
denied; asked again once the release has come): a stopping agent first
sees its running operations through, and the upgrade would wait for that,
for hours maybe. And from the moment an update is decided until its run
on the NAS ended, no job starts (`Jobs` asks the update before it records
one: backups, restores, deploys and image updates, copies, pool changes,
a snapshot's restore, every kind) nor does a scrub: they are refused with
`update_running` (409). A job asked to start counts as running from that
moment, so of a job and an update that start together one sees the
other. Schedules that come due meanwhile do not fail: backups, image
updates and snapshots stay due and run once the update is done, a
certificate's renewal the next day, a share's walk to share-level access
five minutes later. The
record of the latest run is replaced by the next one; an update's goes to
`update.json` as well, which only the next update replaces, so a check or
a plugin installed right after an update leaves how it went for the page
and for its notification, which zendiloosd sends once and keeps in its
database too (for a NAS whose last update a runner without that record
made). Tasks
are fixed: install Docker (`docker.io`, `docker-compose`), the ACME
client (`acme.sh`) and libvirt with QEMU for virtual machines (4.13), the
App Center's plugins; check (lists fetched, then
`apt-get --simulate upgrade` parsed into packages, security updates marked
by Debian's security archive); update; and finish an update. And
systemd-resolved, for taking the network over from ifupdown
(`install-networkd`; systemd-networkd comes with `systemd`): its package
switches name resolution to it at once, enabling and starting it and
moving `/etc/resolv.conf` aside for a link to its stub, which nothing would
tell the name servers while ifupdown still runs the network; so the runner
disables and stops it again and moves the file back, and the takeover
switches it on (6).

**An update cut off** (a power cut while DKMS builds ZFS for the new
kernel, say) leaves packages unpacked or half configured. apt then sees
nothing to upgrade and every install fails with "dpkg was interrupted".
Each check and update reports it (`dpkg --audit` lists something:
`interrupted` in the overview), and after an update that failed
zendiloosd looks at once, without waiting for the daily check. The page
then offers to finish the update (`POST /system/updates/finish`,
`system.manage`, audited as `system.update_finish`): a run of the task
`finish` in the packages unit, followed like an update and kept as the
last update, so it survives the restart of zendiloosd that configuring
zendiloOS's own package brings: `dpkg --configure -a`, then `apt-get
--fix-broken install` from the lists it has, then the look at the kernel
modules; it fails while `dpkg --audit` still lists something. It is
refused while background jobs run, as an update is.

**Releases.** zendiloosd reads the repository's releases from GitHub's API
(`[updates]` in `zendiloosd.toml`): the package for this machine
(`zendiloos_<version>_<arch>.deb`, of the architecture zendiloosd itself was
built for, `amd64` or `arm64`; a release holds one for each, a release
without the machine's is not offered), `SHA256SUMS` and its minisign
signature `SHA256SUMS.minisig`. A release's channel comes from GitHub: stable for a
release, test for a pre-release (`v1.2.0-rc.1`), development for the rolling
release `dev` built from every commit on main. Each channel takes the
steadier ones too; only a version newer than the running one, in dpkg's
order, is offered, so coming back from test to stable waits for a newer
stable release. A check picks its release for the channel set when it
ends, so a channel changed while apt fetched its lists counts, and a
release the channel does not take is neither shown nor installed. Before an update zendiloosd downloads the three files to
`/var/lib/zendiloos/updates`, each tried up to four times, after pauses
of 10, 30 and 60 seconds, while the failure may pass (an answer of 5xx,
408 or 429, a connection not made, broken off or timed out; GitHub
answers `504 Gateway Timeout` now and then); only then does the update
fail, naming the file, the reason and the tries. The runner trusts none of it: it copies them
out (never through a link, size-limited), checks the signature with the key
the package brought along (`/usr/share/zendiloos/release-signing.pub`, or
one an administrator put in `/etc/zendiloos`), that the signature's trusted
comment is `zendiloos <version>` of this very package (one key signs every
build, the development build of each commit too), the package's checksum,
its name, architecture (the name's and the package's own `Architecture`
must both be what `dpkg --print-architecture` says) and version, and
installs only a version newer than
the installed one, so an old signed release cannot be brought back. It is
started with the NAS's channel (`run-packages <id> update <package>
<channel>`, zendiloosd sends it along with the release) and takes a build
only of a kind that channel takes, as its version says: `+dev.` only on
development, `~rc.N` or `~beta.N` on test and development, a plain release
everywhere, any other form as a development build. A build without a key
takes no updates of zendiloOS. How releases are made and
signed: `docs/releases.md`. What changed in the version that runs comes
with its package (`/usr/share/zendiloos/changes.md`, `notes` in
`[updates]`), read once when zendiloosd starts; the page shows it and an
available release's notes as Markdown, with HTML in them as text.

**The kernel and ZFS.** ZFS is built by DKMS for each kernel. Should that
fail for a new one (its headers missing, a kernel newer than the ZFS release
supports, a build cut off), a restart into it would come up without the
pools. So the NAS never starts a kernel without its ZFS module while one
with it is there, also when someone restarts it against the advice: GRUB's
default is the entry zendiloOS saves in GRUB's environment block
(`GRUB_DEFAULT=saved`, from `/etc/default/grub.d/zendiloos.cfg`; the
package writes GRUB's configuration anew once, so that it starts the saved
entry), which is the newest kernel by dpkg's order with a `zfs.ko` below its
modules, a newer kernel only once it has its module too, and the newest when
none has one. The entry is named by its id, the one Debian's `10_linux`
gives that kernel among the advanced options
(`gnulinux-advanced-<uuid>>gnulinux-<kernel>-advanced-<uuid>`), never the
first entry, which starts whatever kernel is newest when GRUB's
configuration is written next, nor a recovery mode or another system's. The
agent's binary sets it as `choose-boot-kernel` (`grub-editenv`, outside the
agent's sandbox, which keeps `/boot` read only), and three things run it:
the package's hooks after a kernel or its headers were installed (DKMS
builds ZFS once both are there) or a kernel was removed
(`/etc/kernel/{postinst,header_postinst,postrm}.d/zz-zendiloos`, which sort
after DKMS's hook and GRUB's own `zz-update-grub` and never fail the
kernel's package), so an upgrade by hand or by unattended-upgrades is
covered too; every run of the package runner, a check too, and an upgrade
or its finishing also when it failed, since a kernel's package fails to be
configured when DKMS's hook fails, its later hooks then never run, and
GRUB's configuration lists the kernel all the same once anything writes it
anew; and the package's installation. Where the kernel that is kept was
removed, GRUB would start its first entry: the hook after the removal
chooses again. A machine that does not start through GRUB (systemd-boot,
another loader) is left alone and says so; removing the package unsets the
saved entry, so GRUB starts the newest kernel again, as Debian's does. After
an update the runner still looks for `zfs.ko` below the newest kernel's
modules; if it is missing the run fails with what to do (`dkms autoinstall
-k <kernel>`), saying that the NAS keeps starting the kernel it chose, or,
where zendiloOS could not choose, not to restart, as the pools would not
import. The Updates page says which kernel starts next and why (`boot` in
`GET /system/updates`): the newest, which has its module; an older one, as
the newest has none yet; the newest, as none has one; or that GRUB does not
start the saved entry (its configuration older than the setting:
`update-grub` writes it anew), could not be told, or is not used. A restart
is waiting when it starts another kernel than the running one, and the
administrator starts it. Tested with fixtures of Debian 13's `grub.cfg` in
the agent's tests, and in the second test VM (`scripts/vm-update-test.sh`,
step 6): a newer kernel installed without its headers, so that DKMS builds
no ZFS for it, leaves GRUB's default on the kernel it had also once GRUB's
configuration lists the new one first; it moves to the new kernel once a
module is there, back once it is gone, and stays when the kernel goes.

**Checks and notifications.** With the daily check on (the default), the
NAS looks once a day and says what it found as a notification, each finding
once; installing stays a click. A finished run is announced once as well.
Settings, the last check and what was said live in the database
(`system_update_settings`, `system_update_state`). Verified in the second
test VM (`scripts/vm-update-test.sh`): a check, zendiloOS updating itself to
a release signed with a test key and coming back with the new version, a
changed package and an older release refused; Docker installed as the
App Center's plugin (`scripts/vm-docker-install-test.sh`).

**[Later] The system updated as A/B images** (issue #58). Keeping a kernel
with ZFS keeps the pools; an update can still leave the system unable to
start (a broken package, an update cut off that finishing cannot repair),
and that needs someone at the console. The appliance's image (phase 5,
section 6) makes the whole system the unit of an update, so that it goes to
the side not running and comes back by itself:

- **Layout.** GPT with an EFI system partition, two root partitions, A and
  B (each a read-only image, erofs or ext4, of a fixed size), and one
  writable partition for `/var` (the database in `/var/lib/zendiloos`, the
  runner's records, logs) and the machine's own part of `/etc` (accounts,
  the network's, Samba's and the agent's files), mounted over the image's
  `/etc` so that a new image brings new defaults while the administrator's
  files stay. The pools stay on their disks as now.
- **Images.** Built in CI with mmdebstrap against a fixed snapshot.debian.org
  state (section 6, Reproducibility), with the kernel and its ZFS module
  built into the image, so the device never builds ZFS and an image whose
  module does not load is never published (Open Question 1: the licensing
  of prebuilt modules). Signed as releases are now (minisign over
  `SHA256SUMS`, the trusted comment naming the version), on the same
  channels, with an image's notes as a release's.
- **Update.** The runner, with the checks it makes today (signature, the
  version it was made for, the channel, only newer), writes the image to
  the side not running and its unified kernel image to the EFI system
  partition (systemd-sysupdate's transfer files describe both); systemd-boot
  starts the new side with boot counting (three tries, in the entry's file
  name), and `systemd-bless-boot` marks it good only once a unit of
  zendiloOS's saw zendiloosd answer, the pools imported and the network up.
  A side that fails three times is left: systemd-boot starts the other one
  by itself, and the page says that the update was rolled back and why, as
  far as the journal of the failed start tells.
- **The database.** Migrations stay forward only; a database migrated by the
  new side must still open on the old one. The copy taken before each
  migration (section 6) goes back in place when the old side starts after a
  failed update, and a release whose migrations an older side cannot use
  says so, so the page can tell that going back loses what changed since.
- **What changes for apt.** Debian's packages are no longer upgraded in
  place on the appliance: security updates come as images, at least weekly.
  The installation from the ISO with apt (phases 1–4) keeps updating in
  place, with the kernel kept as above.

Open: the size of the root partitions, whether the machine's `/etc` is an
overlay or a set of bind-mounted folders, downloading deltas instead of
whole images, and how a machine installed from the ISO moves to the
images.

### 4.11 Certificates and the web interface

**The ACME client is a plugin** of the App Center, beside Docker: acme.sh
from Debian, installed by the package runner (task `install-acme`, as
Docker's) when an administrator asks. Its context, `zendiloos-certificates`,
keeps three things in the database (migration 0033, which also holds the
web interface's settings in `web_server`): **accounts** at an ACME
server (Let's Encrypt, or its staging server for trying out) with a contact
address (its domain kept in lowercase, and only characters acme.sh is
handed); **challenges**, how the server is shown a name is ours: DNS-01
through a DNS provider's API (a curated list of 81 of acme.sh's `dnsapi`
scripts with their variables, from dynamic DNS for a home connection, the
hosters and registrars of Germany, Austria and Switzerland and the large
clouds to a DNS server or panel of one's own; a test checks each name against
what Debian's scripts read, `zendiloos-certificates/tests/acme-sh-dnsapi.txt`
from `scripts/acme-dnsapi-names.sh`; left out are scripts that need another
program, gcloud, OpenStack, nsupdate and knsupdate, or a key file, TransIP
and Oracle Cloud), or HTTP-01
through a file nginx serves on port 80; and **certificates**: names (a
wildcard only with DNS), key (ECDSA P-256 by default, RSA 2048 or 4096),
account, challenge, whether it renews itself, and what came of the last try
with acme.sh's lines. A provider's secret values are stored like the mail
server's password and never sent back: the interface shows
that one is set, and an empty field keeps it.

**acme.sh never runs as root, and never in the agent's sandbox.** It needs
the network, which the agent has none of, and it is a shell script. For
`IssueCertificate` the agent starts a transient unit (`systemd-run --wait
--pipe`) as the system account `zendiloos-acme`, with a sandbox of its own
(`ProtectSystem=strict`, writable only `/var/lib/zendiloos-acme`, no
capabilities, `NoNewPrivileges`, only IP and Unix sockets) and fixed
arguments made of checked protocol values: `--issue --force --server …
--accountemail … --config-home accounts/<n> --cert-home certificates/<n>
--keylength … --domain … (--dns <script> | --webroot webroot)`, each
account with acme.sh settings and key of its own. The DNS API's variables
reach it only in an environment file in `/run/zendiloos-acme` that only
root reads (systemd's `EnvironmentFile=`), so they appear on no command
line and in no unit property; the file is removed when acme.sh ends. Names
of variables that steer programs (`PATH`, `LD_*`) or acme.sh itself (`LE_*`,
`ACME_*`) are refused while the request is read, and the script must be one
acme.sh brings. acme.sh's lines reach the job as they come; acme.sh's own
renewal (a cron job) is never set up. Once issued, the agent copies the
chain and key, size-limited, to `/etc/zendiloos/web/certificates/<n>`
(root's; the key 0600). It opens them folder by folder without following a
link (acme.sh's account owns those folders) and without waiting (a pipe in
place of a file is refused). The pair goes to `<n>.zendiloos-new` first, on
disk, is checked there (the chain holds certificates only, at most ten, as
nginx reads every block of it; openssl reads each, `openssl verify
-partial_chain` finds each issued by the one after it, and the key's public
key is the first's) and only then takes the place of the pair served, in one
step (`RENAME_EXCHANGE`). There `nginx -t` reads it, under the lock changes
of the web server take, and if nginx refuses it, the pair from before
returns, in one step too: whatever fails, nginx finds the working pair from
before, never half a pair. acme.sh's folder of a
removed certificate, and of a removed account (its key at the authority),
is removed by its account, in a unit of its own: the agent may read anything
but write only what is root's. Numbers of accounts and certificates can come
again after one was removed, so a new one first has whatever acme.sh and
nginx kept under its number removed; it is not made when that fails. One
issuance of a certificate runs at a time, and it is not deleted while one
runs.

**Renewal.** zendiloosd looks once a day (the first time five minutes after
its start) and renews, each in a job, every certificate set to renew itself
that ends within 30 days, unless a try failed in the last 20 hours. A
renewal tells: `certificate_renewed` (success) or `certificate_renewal_failed`
(error, with acme.sh's last complaint); issuing by hand does not. A renewed
certificate the web interface shows, or a change of it waiting for
confirmation would, is taken by nginx at once (`nginx -t`, reload, under the
lock the agent changes the web server with); when nginx does not take it,
that is told (`certificate_not_taken`, an error, also when issued by hand)
and the job fails, rather than calling it renewed. The certificate is then
recorded as not served (`served`, migration 0045, with why as its last
error), and each daily look has nginx take it again, told as renewed once it
does, rather than leaving the one before until the next renewal.

**The web interface's settings** (Control panel → Web interface): the HTTP
port, whether plain HTTP redirects to HTTPS or is not served, the HTTPS port,
and the certificate: the machine's own, self-signed one (made anew for the
new name when the machine is renamed, 6), or an issued one that holds. Plain
HTTP never serves the interface itself (the session cookie
is HTTPS-only): it points to HTTPS and answers HTTP challenges from acme.sh's
`webroot`, which needs it on port 80 (as plain text whatever the name, and
never through a link: `types { }`, `disable_symlinks on`). Ports are 1 to
65535, not those of the file services, discovery or SSH, also while they are
stopped (22, 111, 137–139, 445, 2049, 3702, 5353, 5355, 5357, and NFS's
helpers on 20048, 32765–32767), not the same twice, and a new one must be
free (`/proc/net/tcp*`); an SSH port moved elsewhere is caught only while
sshd listens on it. nginx's site includes
two files the agent writes, `/etc/zendiloos/web/http.conf` (the HTTP server,
or nothing) and `https.conf` (the HTTPS server's `listen` lines and
certificate); postinst writes the defaults (80 to HTTPS on 443, the own
certificate). A change works as a network change does: the files it replaces
are kept beside them, `nginx -t` checks the new ones (in a transient unit: it
opens nginx's log and temporary folders) and nginx reloads; unless the
browser confirms within 90 seconds, from the new address if it moved, the
old files return, by a timer and after a restart of the agent (holding the
lock changes take, so no change begins while they return). zendiloosd
saves the settings as it confirms them (first, and takes them back if the
agent cannot keep the change), so the database never holds settings that
went back; applying and confirming take turns under a lock of their own, so
of two confirmations sent at once one gets through and the other takes
nothing back. Had zendiloosd stopped between saving the settings and the
agent hearing of the confirmation, the host went back to the ones before:
at its start zendiloosd waits until no change waits on the host and puts the
settings saved in force again, for good (they were confirmed from the
browser at their address; `ApplyWebServer` without a time to confirm in,
which leaves nginx alone while the files say them already).

### 4.12 Firewall

**One list of rules**, kept by `zendiloos-firewall` (migrations 0035, 0039): each
rule names a network connection (or any), what it is about (all ports, the
web interface, SMB, NFS, SSH, network discovery, ping, a Docker project, or
ports by number with TCP, UDP or both), a source (anyone, an address, a
network, a range; IPv4 or IPv6) and allow or deny. For a new connection the
first rule that matches decides; when none does, the fallback, "allow" by
default, so turning the firewall on changes nothing until there are rules.
The services stand for the ports in use: the web interface's HTTP and HTTPS
ports (also those of a change waiting for confirmation), the SSH server's,
SMB's (TCP 139, 445, UDP 137, 138), NFS's (2049, rpcbind on 111, and mountd,
statd and lockd on the ports the package fixes in
`/etc/nfs.conf.d/zendiloos.conf`: 20048, 32765, 32767, all below the range
Linux hands out for outgoing connections), discovery's
(WS-Discovery, mDNS, LLMNR), and a project's published host ports while its
containers run.

**nftables, a table of its own.** zendiloosd compiles the rules into plain
matches the agent checks once more (interface names, addresses, networks
with their host bits zero, port ranges) and writes as
`/etc/zendiloos/firewall/zendiloos.nft`: the table `inet zendiloos_firewall`
created, deleted and made anew in one transaction, so nothing else (Docker's
tables, Debian's) is touched. Its `input` chain accepts answers, the
loopback, Docker's bridges (containers reach the NAS; `docker0` and the
`br-` bridges the network context finds, each by its exact name, so a bridge
of the machine's own such as `br-lan` meets the rules), what IPv6 needs and
DHCP's answers, then the rules, then the fallback. Its `forward` chain lets
containers' own traffic pass and sends what Docker forwards to a published
port (`ct status dnat`) through the rules that are about all ports, a project
or ports, matched by the port it was sent to (`ct original proto-dst`), then
the fallback; any other new connection from outside to a bridge (a LAN host
routing to a container's address through the NAS) is dropped. That drop knows
bridges by the kernel's kind of device (`meta oifkind "bridge"`), not by
name: every bridge but the machine's own (those the network context finds
with a member besides containers' `veth`s: a network card, a bond, a VM's
`tap`), so also one Docker made after the rules were written or named
otherwise (`com.docker.network.bridge.name`); containers on such a bridge
still reach each other. Frames a bridge of the machine's own forwards
between its ports (a VM's tap and the card) come through this chain too,
once `br_netfilter` is loaded, with the bridge as both interfaces: they
pass, as neither the accept for other bridges nor the drop matches a
bridge listed as the machine's own, and one not listed yet (made since)
meets that accept first. A drop there is final; an accept never overrides
Docker's own drops. The agent has no network of its own (`PrivateNetwork`): `nft -c`
checks the file in a transient unit, and `zendiloos-firewall.service`
(`nft -f`, early at boot before `network-pre.target`, after Debian's
`nftables.service`, and again when that restarts or reloads, as its
`/etc/nftables.conf` flushes everything) loads it, reloaded after each
change. Off, the file only
removes the table; removing the package removes it too.

**Docker and the machine's own bridges: the one place zendiloOS touches
Docker's tables.** Debian 13's `docker.io` (26.1.5) loads `br_netfilter`
as it starts (`modprobe bridge br_netfilter` while it manages iptables),
which hands frames a bridge forwards between its ports to the IP
firewall's hooks (`net.bridge.bridge-nf-call-iptables` and `-ip6tables`
are 1 once the module is there); and whenever it has to switch IPv4
forwarding on, as on a NAS that routes nothing, it sets the policy of
iptables' `FORWARD` chain to `DROP` (also after a firewalld reload). Its
IPv6 tables it leaves alone unless `ip6tables` is turned on in
`daemon.json` (in 26 an experimental option; from 27 on by default). A
frame from a VM's tap to the LAN on a bridge then meets Docker's chain,
which has nothing for it, and is dropped; an accept in
`inet zendiloos_firewall` cannot change that. Docker 28 added the daemon
option `ip-forward-no-drop`, which Debian 13 does not have, and which would
leave the policy open for everything forwarded, not only the bridges'.
Docker keeps the chain `DOCKER-USER` for others' rules: `FORWARD` jumps to
it first, Docker makes it if it is missing and never flushes or changes
what is in it, and its documentation lets forwarding between interfaces
pass there (`-i <if> -o <if> -j ACCEPT`). So `zendiloos-docker-bridges.service`
(the agent's binary as `docker-bridges`) keeps exactly one rule in
`DOCKER-USER`, a jump to a chain of zendiloOS's, `ZENDILOOS-BRIDGES`, which
accepts what comes in on one of the bridges zendiloOS made and goes out on
the same (`-i br0 -o br0 -j ACCEPT`), and nothing else: frames between two
bridges, from a bridge to Docker's containers or the other way still meet
Docker's rules. The chain is filled anew in one transaction
(`iptables-restore --noflush`), for IPv4 and, where Docker made
`DOCKER-USER` there too, IPv6; with no bridge of zendiloOS's left, the jump
and the chain go, and Docker's tables are as Docker made them. It runs
`iptables` as Debian chose it (`iptables-nft`), the same Docker runs. The
unit is wanted by and part of `docker.service`, ordered after it, so it
runs after every start and restart of Docker; the agent starts it after
every change of the network's devices, their undoing, and as it starts;
without Docker it does nothing. Removing the package removes the jump and
the chain, as it removes the firewall's table. A bridge made otherwise
(netplan, by hand) is its owner's to let through.

**Internal networks: the NAS keeps itself off them.** A bridge zendiloOS
made without members whose own file gives the NAS no address (neither
DHCP nor a fixed address, neither router advertisements nor a link-local
address) is an internal network for VMs (4.13). No address alone does not
keep the NAS off it: Linux answers ARP for any of its addresses on every
interface (`arp_ignore` 0) and takes in what comes on one for any of its
addresses, so a VM on the bridge could reach the NAS's LAN address
through it, its services by any rule that allows any interface, and, with
IP forwarding on (Docker turns it on), be routed further; replies go out
by the NAS's routes, but a connectionless packet needs none. So the agent
writes `/etc/zendiloos/network/internal-networks.nft` from its own
systemd-networkd files, a table of nftables' `bridge` family,
`bridge zendiloos_internal`, created, deleted and made anew in one
transaction: its `input` chain drops every frame such a bridge hands up
to the NAS (`meta ibrname`), its `output` chain every frame the NAS would
send onto one (`meta obrname`); without an internal network it only
removes the table. Frames between the VMs' ports are the bridge's own and
meet neither hook; with `br_netfilter` they pass the IP forward chains
with the bridge as both interfaces, where this firewall's `forward` chain
lets them through (the bridge is one of the machine's own once a VM's tap
is in it, and one not listed yet meets the accept for other bridges
first) and Docker's FORWARD chain does through `ZENDILOOS-BRIDGES`, which
lists every bridge zendiloOS made. `zendiloos-internal-networks.service`
loads the file early at boot, before `network-pre.target`, after Debian's
`nftables.service` and again whenever that restarts or reloads, as
`zendiloos-firewall.service` does; the agent writes it and has it loaded
after every change of the network's devices, their undoing, and as it
starts, also while the firewall is off. Removing the package removes the
table. A card goes on an internal network only while the rules in force
list it (4.13).

**Changes go back unless confirmed**, as network and web server changes do:
the file replaced is kept as `zendiloos.nft.previous` and returns after 90
seconds, by a timer in the agent and after its restart. While a change
waits, what others send on connections opened before it meets the rules too,
in both chains (only answers pass at once), so the confirmation, which a
browser sends on the connection it has open, gets through only if the new
rules let that browser in, also when that connection goes to a reverse proxy
in a container and is forwarded; once confirmed, the rules are written again
without that. zendiloosd saves the settings only once confirmed, and keeps
a change waiting in memory only: after its restart nobody can confirm it,
and the first refresh ends it with the rules kept in force. Every refresh
names the change it began, if any, and the agent
decides under the same lock as its timer, so rules for a change that just
went back are never put in force again. It also refuses settings that would keep out
the browser asking for them (its address from `X-Real-IP`, the interface
from the address nginx was reached at, `X-Server-Addr`, and the port,
`X-Server-Port`), unless the request says that is meant. Locked out anyway,
`nft delete table inet zendiloos_firewall` on the console lifts the rules
until the next change or restart.

**Kept in step.** zendiloosd puts the rules in force at its start, when a
job ends (a project deployed, stopped, removed), when SSH, the file services
or the web interface's ports change, and every 30 seconds, asking the agent
only when the compiled rules differ from those it gave last. What cannot be
read (Docker not answering) stays as last seen, so a hiccup never drops a
project's ports; the ports last seen are kept in the database too, so a rule
about a project holds from zendiloosd's start, before Docker answers. A service that is on but that the rules let nobody reach (an
"allow" for any of its ports first means someone gets in; a "deny" of all of
them to anyone on every connection first, or a denying fallback, means
nobody does) is listed on the Firewall tab with a way to allow it, and told
once as `firewall_blocked` until it is reachable again.

**After a configuration backup is restored** the backup's settings are not
put in force at the start, as every other start does with the settings
kept: they could be another network's and keep the administrator out,
with nobody able to confirm them. They are set aside (migration `0060`),
the machine starts with the defaults (off), and the Firewall tab offers
them, applied as any change and kept only once confirmed (4.15).

### 4.13 Virtual machines

**What it is.** A third plugin of the App Center, *Virtual machines*:
libvirt and QEMU from Debian, installed by the package runner when an
administrator asks, the VMs' storage on a pool, and then a desktop app of
that name in which administrators (`vms.manage`) make, change, start, stop
and remove VMs, keep a library of ISOs, watch what the VMs use and open
their consoles. The app has a page for the machines (how many run, the
share of the NAS's CPU they keep busy, the memory the running ones were
given against the NAS's, and a card per VM with its own load) and one for
the image library. Every value is checked as in the other contexts,
nothing a client sends reaches a command line or a domain's XML as text,
and zendiloosd gains no privilege. Its context is `zendiloos-vms`
(migrations 0049, 0050, 0051, 0060, 0061 and 0062). Besides Machines and
Images the app has pages for the VMs' storage, their networks, their
backups and their log; a VM's card opens its snapshots.

**The hardware check.** The plugin's page says whether the CPU can run VMs
(`/dev/kvm` is there, with the flag `vmx` for Intel VT-x or `svm` for
AMD-V in `/proc/cpuinfo`), whether an IOMMU is on (`/sys/kernel/iommu_groups`
lists groups: needed to hand a PCI device to a VM), with what the kernel's
command line says of it (`intel_iommu=`, `amd_iommu=`, `iommu=`) and how to
switch it on for the CPU's maker (VT-d or AMD-Vi in the firmware, and on
Intel `intel_iommu=on` in `GRUB_CMDLINE_LINUX` of `/etc/default/grub`,
`update-grub`, a restart), and whether swtpm and OVMF with Microsoft's keys
are there, each with a line of explanation; the agent reads them (`VmHost`,
`VmHostDevices`), as it reads the machine's other facts, so the simulation
can give them too.
Without KVM the installation is refused (`kvm_missing`): QEMU would run,
but at the speed of an emulator. VMs are offered on amd64 only for now:
the domains zendiloOS writes are a PC's (`q35`, OVMF, `x86_64`), and an
arm64 NAS would need QEMU's `virt` machine with AAVMF instead. There the
page says so (`architecture`, `architecture_supported` in the plugin's
`hardware`) and the installation is refused (`vms_architecture`), by the
package runner too, so QEMU for x86 never comes onto an arm64 NAS.

**libvirt, not QEMU run by hand.** libvirt starts QEMU as `libvirt-qemu`,
confined by an AppArmor profile it makes for each VM (`virt-aa-helper`:
only that VM's disks, the images in its drives and its console socket), hands the files it attaches
to that account while the VM runs and gives them back after, starts passt,
picks the OVMF build from the firmware QEMU describes, shuts a guest down
through ACPI and starts VMs with the machine. zendiloOS talks to its system
instance (`qemu:///system`) through `virsh`, with fixed arguments, and names
only domains `zendiloos-<id>`, the id twelve hexadecimal digits zendiloosd
draws at random: the agent never lists into its answers, changes or removes
a domain without that prefix, and zendiloosd ignores any other in
`virsh list --all`, the one look at their states (raw, read unprivileged).
A VM's name is zendiloOS's alone and never reaches libvirt.

**The packages, and no network of libvirt's.** The task `install-vms` runs
`apt-get install --no-install-recommends libvirt-daemon
libvirt-daemon-driver-qemu libvirt-clients qemu-system-x86 qemu-utils ovmf
passt swtpm swtpm-tools`. A plugin installed before VMs had a TPM lacks
swtpm: its page offers to install again, which brings it. Not `libvirt-daemon-system`, as one would expect: in Debian 13
(libvirt 11.3) it *depends* on `libvirt-daemon-config-network`, which
defines libvirt's default network (the bridge `virbr0` with
192.168.122.0/24, its own dnsmasq and NAT rules it writes into the
firewall), on `libvirt-daemon-config-nwfilter` and on the network and
nwfilter drivers; and the daemon *recommends* every driver. Without them
libvirtd has no network driver at all, so nothing of libvirt's ever makes a
bridge, starts a DHCP server or writes a firewall table: the firewall of
4.12 stays the only one. What `libvirt-daemon-system` would also do,
enabling the daemon, the agent does when the storage is set up
(`virtlogd.socket`, which QEMU's output goes through, `libvirtd.socket`
and `libvirtd.service` enabled, the daemon restarted);
Debian 13 runs the monolithic `libvirtd`. The QEMU driver's own
dependencies bring `systemd-container`; its recommendations (swtpm, passt)
are in the list above.

**The storage, in locations.** Setting up puts the first location, the
dataset `<pool>/zendiloos.vms` on a pool zendiloOS made (`setuid`,
`devices` and `exec` off; the dot keeps it from ever being a share),
mounted by its own unit at `/volume<N>/@vms` before libvirtd, which a
drop-in (`RequiresMountsFor`, one line per location) keeps from starting
without every location, as for Docker's data. More locations go on other
pools the same way (`POST /vms/locations`, `vms.location_add`), one per
pool; zendiloosd keeps them in the order they were added (migration 0051,
`vm_locations`, which took the pool the VMs of before were on as the
first, and takes in one the host has and the records not). The first
holds the image library, `/volume<N>/@vms/isos`, and stays where it is.
A VM's disks are sparse zvols `<pool>/zendiloos.vms/<id>-disk<N>`
(`volmode=dev`: their partitions never show on the host), each on the
location it was made on: the first unless another is chosen, in the
wizard and the edit dialog; a disk never moves (zendiloosd and the agent,
which looks for the zvol of that slot on every location, refuse), and a
VM's disks are destroyed on every location when it goes. The Storage page
(`GET /vms/locations`) shows each location with its pool's health and
space and the VMs with disks there, each with what its zvols take
(`used`) and hold (`referenced`), read with one `zfs list` of the
locations' datasets. A location goes (`POST
/vms/locations/{pool}/delete`, `vms.location_remove`) only with the
pool's name typed again, never while a VM has a disk there or the library
images, nor the library's while there are others (`location_in_use`,
naming them); the agent checks again that the dataset holds no zvol and
the library no image, has libvirt stop waiting for it first (so stopping
its mount never stops libvirt), unmounts it (a mount in use stays, and
so does everything), destroys the dataset and removes its unit, putting
back what it did should a step fail. A pool with a location on it is not
deleted: the storage context refuses (`pool_in_use`, "the virtual
machines' storage", from `vm_locations`), and so does the agent, by the
location's mount unit; moving a disk to another location is not offered
yet.

**A VM.** Its settings: a name, an OS preset (Linux, Windows, other) that
fills in vCPUs, memory, the first disk's size, the firmware, the disks'
bus and the cards' model, vCPUs up to the machine's threads and memory up
to its total (the administrator weighs what the NAS keeps), UEFI (OVMF,
with Secure Boot if chosen) or BIOS (SeaBIOS), a TPM 2.0 or none, and its
devices: 1 to 8 disks, each in
a slot (0 to 7, its zvol `<id>-disk<slot>`) on a location, with a size of
1 GiB to 64 TiB and a bus, virtio (Linux) or SATA (systems that bring no
virtio drivers, Windows' installer among them); up to 4 network cards,
each in a slot, virtio or an Intel e1000e every system knows, through
passt, on a host bridge, directly on a wired card or bond, or on an
internal network; up to 3 SATA CD drives, each with an image of the library or
empty (an image goes into an empty drive while the VM runs, which needs
the drive); and a boot order of these disks and drives, each once, the
others never booted from. A new VM gets one disk, one card through the
NAS and one empty drive, booted from the drive first. Each card gets a MAC
address when it is added (`52:54:00`, QEMU's prefix, the rest at random,
none another card has) and keeps it whatever else changes, so a DHCP
server's reservation holds; a client never chooses one. The agent builds
the domain's XML from the checked protocol types alone (`VmSpec`, read
only whole: slots, MAC addresses and boot devices each once, the boot
order of devices the VM has) and keeps it as
`/etc/zendiloos/vms/zendiloos-<id>.xml` (`virsh define --validate`; written
beside it first and kept only once libvirt took it, so the file is always
libvirt's definition): q35, the host's CPU, ACPI; each disk on a target of
its slot (virtio `vda` to `vdh`, SATA `sda` to `sdh`) and each drive after
them (`sdi` to `sdk`), so a device keeps its place whatever is added;
`<boot order>` on the devices of the boot order; for Linux virtio video,
a memory balloon whose guest reports its memory every five seconds and a
random source; VGA and no balloon for the others; Hyper-V's enlightenments
and local time for Windows; a VNC console on a Unix socket and nothing
listening on a network. Making a VM is a job (`vms.create`): its disks
made, the domain defined; what a failure leaves goes again, its record
too. Start, shut down (ACPI), restart (ACPI), force off, autostart on and
off, a change, and delete with the name typed again (the domain undefined
with its NVRAM, its zvols destroyed; the images in its drives stay in the
library) are audited (`vms.install`, `vms.storage`, `vms.create`,
`vms.start`, `vms.shutdown`, `vms.restart`, `vms.force_off`,
`vms.autostart_on`, `vms.autostart_off`, `vms.change`, `vms.delete`,
`vms.console`, `vms.location_add`, `vms.location_remove`). The app's Log
page shows these entries (`GET /logs/audit?prefix=vms.`): who, when,
what, outcome, searched on the server (`search`: what actor, action,
target, address or reason hold, as plain text), a page at a time and
exported as CSV in the browser (every field quoted, none starting with
what a spreadsheet takes for a formula); the control panel's audit log
has the same table. Nothing of its own is stored for it.

**Changing a VM.** Everything but its system can be changed after it is
made (`PUT /vms/{id}`, the whole settings, `vms.change`), and the domain is
always defined anew from the stored settings by the agent, never edited as
text. Its name (zendiloOS's alone) and autostart change at any time; so do
the images in its drives: the domain is defined anew and, while QEMU runs,
the medium changed at once (`virsh change-media --update` or `--eject`,
`--live --force`, as a guest may have locked its tray); libvirt's security
drivers give the new file to QEMU's account and its AppArmor profile, as
at a start. Everything else is hardware and changes only while the VM is
shut off: vCPUs, memory, the firmware (the interface warns that a system
installed with one may not boot with the other), disks added, grown or
removed, cards added, removed or changed, drives added or removed, the
boot order. zendiloosd refuses it otherwise (`vm_running`), and so does
the agent (`UpdateVm`), which looks at libvirt's state itself. A disk only
grows (`zfs set volsize`; the guest grows its partition itself), never
shrinks; the agent looks at every zvol's size first and changes nothing if
one would. A disk removed is destroyed with its data, after the domain no
longer names it, and only with the VM's name typed again. A card keeps
its MAC address by its slot, from the VM's record as it is once the
change holds the lock that changes and creations take (read again then,
not from a look before the host was asked); a card on a bridge, internal
network or card that is gone since may stay on it, but none goes onto one
the machine does not have. A VM is read with its devices in one
transaction, so a record is never seen with the devices of a change
written meanwhile, and zendiloosd's journal names the cards of each VM
recorded and changed (`vms: recorded`, `vms: changed`, slot and address
each). Devices are kept in the order of their slots, so a change that
sends them in another order is no hardware change. (Issue #71, cards with
other addresses after an edit in two of about ten runs of the VM test,
has no cause found in the code: a test changes VMs at random, the cards in
any order, others made and changed at the same time, and every card keeps
its address.)

**The network.** A card's network is one of four kinds, which the
Networks page lists with the VMs on each and what each means in one line.
Each card goes through passt by default, user-mode
networking: the VM's connections leave from the NAS's own address as the
NAS's sockets, through
its firewall's rules for what goes out; nothing reaches the VM (no ports
are forwarded). Or a Linux bridge of the machine's own (not Docker's
`docker0` or `br-<id>`, not libvirt's `virbr…`), on which libvirt puts the
VM's tap: the VM is on the LAN as a machine of its own and gets its address
from the LAN's DHCP server. Such a bridge is made on the Network page (6),
over the card that connects the NAS to the LAN, which keeps the NAS's
address; the wizard and the edit dialog offer the bridges there are and,
without one, point there. Which those are zendiloosd reads itself, with
`ip` in the host's network namespace, as the Network page does (the agent has a network
namespace of its own, whose `/sys/class/net` shows only its loopback):
every bridge but Docker's (`docker0`, `br-` and twelve hex digits) and
libvirt's (`virbr…`), also one without members made for VMs only. Defining
a VM with a card on a bridge, the agent checks again that it is one of
the host's links and a bridge, asking systemd-networkd (`GetLinkByName`,
`DescribeLink`'s kind and type); a link it does not know or describes as
anything else is refused, and so is every bridge where systemd-networkd
does not run (ifupdown, NetworkManager). A link it knows but does not
describe is taken as it is, as libvirt refuses to start a VM on anything
but a bridge. Frames between the VM and the LAN pass the firewall's forward chain
(4.12) and, where Docker runs, its `FORWARD` chain through zendiloOS's rule
in `DOCKER-USER` (4.12), for the bridges zendiloOS made. A bridge a VM is on
(running or not) is not removed: the Network context asks through its port
`DeviceUsers`, which `zendiloos-infra` answers from the VMs' records, and
refuses with `device_in_use` naming the VMs. The firewall's rules do not
filter what a VM on a bridge sends to the LAN or receives from it, only
what reaches the NAS itself.

*Directly on a card* (`direct`): macvtap on a wired card or bond of the
host's in bridge mode (`<interface type='direct'>`, `<source dev='<card>'
mode='bridge'/>`), which libvirt's QEMU driver makes itself, without a
network driver: the VM is on the card's LAN as a machine of its own,
without a bridge, and gets its address there. The NAS and such a VM do
not reach each other: the card hands the frames a VM sends out to the
wire, never back to the NAS's own stack, and the NAS's frames to the VM's
address go out on the wire too (a switch does not send them back); this
is how macvtap works, and the page says so plainly. Offered are the
host's wired cards and bonds in no bridge or bond (zendiloosd's view with
`ip`); the agent checks again through systemd-networkd that the link is
of the type `ether` without a kind (a card; a VLAN or a `veth` has one)
or of the kind `bond`, refusing a link it does not know or describes as
anything else, and taking one it knows but does not describe as it is.
What the firewall sees of it: the VM's frames leave by the card's own
transmit path and come in for its MAC address by the macvlan receive
handler, before the NAS's IP stack, so they meet neither the firewall's
`input` nor its `forward` chain, nor Docker's FORWARD chain (no bridge, so
no `br_netfilter`): its traffic is not filtered by the NAS, as a bridged
VM's is not, and needs no rule in `DOCKER-USER`. A card a VM is directly
on is not taken into a bridge or bond on the Network page: the Network
context asks its `DeviceUsers` port before, and refuses with
`device_in_use` naming the VMs, as for removing a bridge.

*On an internal network* (`internal`): a bridge zendiloOS made without
members whose own file gives the NAS no address, which the NAS keeps
itself off (4.12, a table in nftables' `bridge` family): VMs on it reach
each other, neither the LAN nor the NAS. It is made on the Networks page
through the Network context's own device API (`POST /network/devices`, a
bridge without members, as #50 made those for VMs), so the same checks,
files and undoing apply; the page confirms the change at once (`POST
/network/confirm`): a bridge without a member or address changes nothing
the browser reaches the NAS by, and the confirmation still proves the NAS
answers. Removing it is the Network context's too, refused while a VM is
on it. The plugin offers the internal networks apart from the host's
bridges (the Network context names them, `internal` in `GET
/network/setup`); while zendiloOS's files cannot be read, no bridge is
offered, so none is taken for the LAN's. On the domain it is a bridge as
any other; defining it, the agent checks that it is a bridge of the
host's and that the rules in force keep the NAS off it (the file is that
of the bridges its files make internal, and its unit is active), else
refuses. A bridge that gets a card on the Network page is no internal
network any more (the LAN's), and its VMs' cards stay on it; one that is
gone keeps the cards that were on it, as a host bridge does.

**The image library.** ISOs live once in `/volume<N>/@vms/isos` as
`<image>.iso`, the image's id twelve hexadecimal digits zendiloosd draws,
and as records (migration 0050: a name of up to 128 characters, unique
without case and renamable, the size, where it came from, when it was
added); as many VMs as want one have it in a drive, read-only, and an
image is not deleted while a VM has it (`image_in_use`, naming the VMs).
QEMU must not reach the shares, and libvirt, which gives each file it
attaches to QEMU's account while the VM runs, must never change a file in
one: only the library's files are ever attached. An image is added in one
of two ways, each audited (`vms.image_add`, `vms.image_upload`,
`vms.image_rename`, `vms.image_delete`).

*From the NAS*, in a job (`vms.image_add`), with its progress. zendiloosd
first opens the file through the file manager as that administrator
(`SharesForVms`), so a share or a file they may not read is refused
(`iso_denied`). The system agent cannot write the volumes, so it starts
the unit `zendiloos-vm-iso-<image>.service` (this binary as
`copy-vm-iso`), which may write only `/volume<N>/@vms` and has no
capability but `CAP_DAC_READ_SEARCH`. It opens the file beneath the
share's root (`openat2`, no links, no other file system, only a regular
file, without waiting on a pipe) and copies as `cp --reflink=auto` would:
`FICLONE` first, then `copy_file_range`, which OpenZFS 2.3 answers by
cloning blocks within the pool where its `zfs_bclone_enabled` lets it
(cheap and instant), else a plain copy; under a hidden name, renamed once
complete and on disk. Whoever may write the share could swap the file
between the check and the copy for another plain file in that share,
which would be copied in its place; administrators are as good as root
anyway (S2).

*From the browser* (`POST /vms/images/upload`, the body as it is, of the
length `Content-Length` announces, at most 32 GiB, which nginx passes on
unbuffered). zendiloosd must not gain write access to the VMs' storage,
so it never holds anything of it, not even a descriptor of a file there:
the agent (`OpenVmImageUpload`) checks the size against the limit and the
pool's free space, makes a pipe, and starts the unit
`zendiloos-vm-upload-<image>.service` (this binary as `receive-vm-image`,
`systemd-run --pipe` with the pipe's reading end as its standard input),
which may write only `/volume<N>/@vms`, has no capability at all and runs
for twelve hours at most; zendiloosd gets the pipe's writing end with the
answer (`SCM_RIGHTS`, as a console's socket). The unit reads the pipe
into a file without a name in the library's folder (`O_TMPFILE`; a
hidden name where a file system has none), counting: past the size
announced it stops and refuses. Only once the pipe is closed after
exactly that many bytes is the file written out (`fsync`) and given its
name (`linkat` of `/proc/self/fd/<n>`, which needs no capability for a
file of its own), which may not be taken; until then nothing of it shows
in the folder, and an upload cut off, refused or given up (the browser
gone, zendiloosd or the agent restarted) leaves nothing, as the nameless
file goes with its last descriptor. zendiloosd writes what comes and
stops at the size announced, closes the pipe and asks for the end
(`FinishVmImageUpload`), which the agent answers from the unit's last
line; a refused or failed upload's record goes, and its file, if any.
What a pipe holds at a time is all a compromised zendiloosd could put
anywhere: the unit, not zendiloosd, decides what becomes a file and of
which name. Images being copied or uploaded when zendiloosd stopped are
removed at its next start, with what of them is on the host.

The proof of concept copied a VM's ISO to `isos/<vm id>.iso` beside the
others: migration 0050 makes each such copy an image of that id, named
after its file in the share (numbered where two have the same name), its
size looked at the first time the library is listed. Each VM it migrates
gets one disk (`<id>-disk0`, of its bus by its preset), one card (its MAC
address made from its id, so it changes once from the one libvirt had
given it), and one drive with its ISO or empty, booted as it was.

**What VMs use.** Every three seconds while someone with `vms.manage`
watches, the live WebSocket's topic `vm_stats` (3.1) carries each running
VM's CPU (its busy share of its vCPUs, from the CPU time between two
looks), the memory it uses (as its guest says through the balloon
driver: in all less what it can use without swapping; else what QEMU
holds of the host's, at most what it was given) and its network's
receive and send rates, and the VMs' share of the machine: of its
threads, and the memory the running ones were given against its total.
The agent asks libvirt once per round for all domains (`virsh domstats
--list-active --nowait --cpu-total --balloon --interface`, never waiting
for a guest that does not answer) and passes on only zendiloOS's domains;
zendiloosd reads the counters, unprivileged. One round serves every
watcher, and none runs while nobody watches. A card through passt has no
device on the host, so libvirt counts nothing for it: its rates show as
not counted.

**The console.** QEMU's VNC server listens on
`/run/zendiloos-vms/<id>.vnc`. The folder is made at every boot by
systemd-tmpfiles from `/etc/tmpfiles.d/zendiloos-vms.conf`, which the agent
writes at setup: `libvirt-qemu`'s, its group root and set-group-ID, so the
sockets QEMU makes there (umask 002) are writable by root's group, which
lets the agent, root without `CAP_DAC_OVERRIDE`, connect; an AppArmor rule
of zendiloOS's in `/etc/apparmor.d/abstractions/libvirt-qemu.d/` (the
abstraction's own folder: Debian's QEMU driver clears
`local/abstractions/libvirt-qemu` as obsolete) lets QEMU use it, besides
the socket's path that `virt-aa-helper` puts in each VM's profile. The agent
opens the socket with `O_PATH | O_NOFOLLOW`, takes only a socket, connects
through its descriptor (`/proc/self/fd/N`), so a link QEMU's account put
there never leads it to another socket (Docker's) that zendiloosd would
then hold, and hands the connected socket to zendiloosd with the answer
(`SCM_RIGHTS`), exactly as it hands over a terminal's master (4.9).
zendiloosd relays it to `/api/v1/vms/{id}/console`: a WebSocket of the same
origin (the upgrade is a GET without a CSRF token), for `vms.manage` only,
the session checked again every minute, pinged every 30 seconds for nginx,
four per account, the subprotocol `binary` as noVNC asks for it, RFB in
binary frames both ways and never a text of zendiloOS's own; why it ended
is said in the close frame (4000 with the reason it could not be opened,
4001 when the session ended, 1000 when the VM's console closed). VNC needs
no password here: the socket is reachable by root and QEMU's account only,
and through zendiloosd by administrators only. In the browser, noVNC 1.7.0
(MPL-2.0) is loaded when a console opens and given that WebSocket as its
channel, scaled to the window, with Ctrl+Alt+Del and full screen; one
console window at a time, whose page is the VM's id.

**noVNC's licence.** `deny.toml` holds the licences of the Rust crates;
CI checks none for the frontend's packages. MPL-2.0 is copyleft per file:
its files may be bundled unmodified into the Apache-2.0 web interface as
long as their licence is kept and their source named, which
`debian/copyright` does (with pako, which noVNC vendors, MIT). A change to
one of noVNC's own files would have to be published under MPL-2.0; none
is made. Decided deliberately, as `deny.toml` asks of any other licence.

**A TPM and Secure Boot (Windows 11).** A VM may have a software TPM 2.0,
swtpm from Debian, as libvirt starts it (`<tpm model='tpm-crb'>` with an
`emulator` backend of version 2.0). Its state stays where libvirt keeps it,
`/var/lib/libvirt/swtpm/<uuid>/tpm2/`: keyed by the domain's UUID, which a
redefinition keeps (4.13, a VM), in `/var/lib`, which no package upgrade
touches, set up and handed to swtpm's account (`tss`) by libvirt with
swtpm_setup and its AppArmor profile; on a VM's location it would need
both done by hand. It goes with the domain (`undefine --tpm`) and comes
with a backup. UEFI may boot with Secure Boot and Microsoft's keys
enrolled: libvirt picks the firmware QEMU describes with `secure-boot` and
`enrolled-keys` (Debian 13's `40-edk2-x86_64-secure-enrolled.json`, that
is `OVMF_CODE_4M.ms.fd` with `OVMF_VARS_4M.ms.fd`), on q35 with SMM on
(`<smm state='on'/>`), as Secure Boot needs; the agent refuses Secure
Boot while no such firmware is described, and a TPM without swtpm. The
Windows preset has both (Windows 11 needs them), the others may switch
them on; Secure Boot needs UEFI. Both are hardware, changed only while the
VM is off. Another firmware or Secure Boot switched leaves the UEFI
variables of the old firmware behind: the agent notes it beside the
domain's XML (`zendiloos-<id>.reset-nvram`) and starts the VM next with
`virsh start --reset-nvram`, so they start afresh from the new template
(boot entries go; Windows boots from its fallback path).

**The host's devices.** A VM may be given up to 8 USB and 8 PCI devices
of the host's (`GET /vms/devices`: USB from `/sys/bus/usb/devices`, hubs
aside, PCI from `/sys/bus/pci/devices` with its IOMMU group, its driver and
its names from the PCI ID database). USB goes by vendor and product
(`<hostdev type='usb'>` with `<vendor>` and `<product>`): that stays when a
device is plugged in elsewhere or again, where its bus and device number
change; libvirt cannot tell two devices of the same vendor and product
apart, so with two plugged in the agent refuses to start the VM and says
to unplug one. USB devices change while the VM runs, plugged in and out
at once (`virsh attach-device` and `detach-device --live`); a new one must
be plugged in, one it had may be unplugged meanwhile. PCI goes by address,
only while the IOMMU is on, only while the VM is off, and through
libvirt's `managed='yes'`: libvirt binds the device to `vfio-pci` when the
VM starts and gives it back to its driver when it stops; nothing is bound
at boot. The devices of an IOMMU group go together: the interface shows
the whole group and adds it at once, and zendiloosd and the agent refuse a
device without the rest of its group (PCI-to-PCI bridges aside, which stay
with the host as VFIO allows). Never handed over, nor any device of its
group: a bridge of the bus, the graphics card the firmware set up for the
console (`boot_vga`), a controller with a disk in use below it (a holder,
mounted, swap, or claimed as ZFS's pools claim theirs: an `O_EXCL` open
fails), and a network card systemd-networkd says the host uses (its link's
`Path` names the device and its `OperationalState` is `routable`,
`degraded`, `enslaved` or `degraded-carrier`), every network card where
systemd-networkd does not answer. Before each start the agent checks again
that every device the domain names is there, a USB device exactly once,
and the PCI devices may still go, and refuses with a clear reason
otherwise. The interface warns that a PCI device leaves the host while the
VM runs.

**Snapshots.** A VM's snapshot is a ZFS snapshot of every disk of it,
`<pool>/zendiloos.vms/<id>-disk<N>@zdl-YYYY-MM-DD_HH.MM.SS` (UTC, as the
shares' are named), taken by one `zfs snapshot` per pool, which takes all
its snapshots in one transaction; its name, the schedules it was taken for
and how consistent it is are ZFS properties (`zendiloos:note`,
`zendiloos:schedule`, `zendiloos:consistency`), so nothing about them is
kept anywhere else. Taken while the VM is off, it is as consistent as it
gets; while it runs, the agent first asks the QEMU guest agent to freeze
the guest's file systems (`virsh domfsfreeze`, thawed right after) and,
without it, the snapshot is crash-consistent, as after a power cut, which
the interface says; a running VM with disks on more than one pool is then
paused for the moment of the snapshots (`virsh suspend`, `resume`), so they
are of one moment. A rollback (`zfs rollback`) is only for a VM that is
shut off, and only to the newest snapshot unless the administrator agrees
that the newer ones go (`zfs rollback -r`; the API refuses with
`newer_snapshots` naming them); each disk keeps the size it has, as disks
only grow, and a disk added since stays as it is. Schedules take them
hourly, daily, weekly or monthly and keep the newest 1 to 100 of each, as
the shares' schedules do (`vm_snapshot_schedules`, migration 0062), looked
at every five minutes; one taken by hand is never let go by a schedule. A
VM's snapshots go with it and with its disks (`zfs destroy -r`). Audited:
`vms.snapshot`, `vms.rollback`, `vms.snapshot_delete`,
`vms.snapshot_schedules`.

**Backups.** A VM is backed up into a share or onto a removable drive
(`POST /vms/{id}/backups`), in a job with its progress, also while it runs:
a snapshot of its disks is taken for the backup (as above, so the backup is
as consistent as that snapshot) and goes again afterwards. Into
`<folder>/<id>/<YYYYMMDD-HHMMSS>/` go each disk as a raw image compressed
with zstd (`disk<N>.raw.zst`), the UEFI variables (`nvram.fd`, libvirt's
`/var/lib/libvirt/qemu/nvram/zendiloos-<id>_VARS.fd`), the TPM's state
(`tpm2-00.permall`) and a manifest (`manifest.json`) with zendiloOS's
settings of the VM (not libvirt's XML, which the agent makes from them),
every file's size and SHA-256 and how consistent it is. A raw image rather
than a `zfs send` stream: any machine reads it back with Debian's `zstd`,
it goes onto a zvol of any pool or into another hypervisor (`qemu-img`),
and a restore never hands a stream from a share users may write to the
kernel's `zfs receive`, only bytes into the guest's own disk; zeros, what
a sparse zvol mostly holds, cost almost nothing compressed. What it costs:
each disk is read whole (holes read quickly), and there are no incremental
backups. The agent makes the snapshots' devices show for the time of the
backup (`snapdev=visible`, then inherited again) and runs the work in a
transient unit of its own (this binary as `back-up-vm`,
`zendiloos-vm-backup-<id>.service`), outside its sandbox: the system read
only but the share or drive, no device but zvols, read only
(`DeviceAllow=block-zvol r`), `CAP_DAC_READ_SEARCH` and `CAP_DAC_OVERRIDE`.
Its files are written under hidden names and made writable by root only,
the manifest put in place last, as Docker's volumes' are (4.9); whoever may
read the share may read the VM's disks, which the interface says. Once
written, the backup is recorded (`vm_backups`, migration 0062) and stays
recorded when its VM goes. A backup or restore can be canceled (`POST
/vms/{id}/backups/cancel`): zendiloosd ends its request, the agent stops
the unit and waits for it (`EndVmWork` makes sure), and the job ends as
`canceled`; meanwhile the VM is neither changed nor deleted nor snapshotted
on schedule, and the agent refuses as long as the unit runs.

A backup is restored (`POST /vms/backups/{id}/restore`) as a new VM, of a
name given, its disks on a location chosen, with new MAC addresses, or
over the VM it came from, which must be shut off, with its name typed
again: it gets the backup's settings back (its disks at least the size they
have, a disk the backup has not destroyed, its cards' MAC addresses as
they were). Images no longer in the library leave their drives empty, a
card on a network the machine no longer has goes through the NAS, and the
host's USB and PCI devices are not given again. The agent defines a new VM
first; over an old one it snapshots its disks first, makes or grows them,
and defines the domain anew only once the files are in. The unit
(`restore-vm`, `DeviceAllow=block-zvol rw`, writable libvirt's folders of
the UEFI variables and the TPM states) opens each file beneath the share's
root without following a link, refuses one others may write, and checks
its SHA-256 and size against the record while it reads; it writes runs of
zeros as holes (`fallocate` punching them, which a zvol frees) and punches
what lies after the image, so a disk stays sparse. Should anything fail, a
new VM goes again with its disks, and an old one's disks return to the
snapshot from before; once restored, that snapshot goes. A backup is
deleted with its files (`remove-vm-backup`, its folder once empty).
Audited: `vms.backup`, `vms.backup_cancel`, `vms.restore`,
`vms.backup_delete`.

**[Later]** Moving the storage to another pool, rules of the firewall for
VMs on a bridge (what is bridged to them is not filtered), counting the
traffic of cards through passt, what libvirt-guests does at a shutdown
(Debian's default: guests shut down), moving a disk or the image library to
another location, a host-only network (an internal network with an
address of the NAS's), a way for the NAS to reach VMs directly on a
card (a macvlan of the host's own), incremental backups of VMs (`zfs
send -i` or changed blocks), backups of VMs on a schedule and to servers
and clouds, finding VMs' backups on a drive again to record them, and PCI
devices hot-plugged.

### 4.14 Backup

**What it is.** A fourth plugin of the App Center, *Backup*: rsync, the
SSH client and rclone from Debian, installed by the package runner (task
`install-backup`: `rsync openssh-client rclone` without recommended
packages) when an administrator asks; then a desktop app of that name for
administrators. Its permission is a new one, `backup.manage` (4.3),
rather than `shares.manage` or `storage.manage`: a backup reads whole
shares whatever their rules say and a restore writes into them, which is
an administrator's alone, and the app is shown by it. Its context is
`zendiloos-backup` (migration 0053).

**Tasks.** A task has a name, a source, a target, a mode, a schedule,
filters and options, of four kinds as its places say:

- *on this NAS*: a folder of a share to a folder of another share (on
  another pool) or of a removable drive;
- *to an rsync server*: a folder of a share to a folder on a server;
- *from an rsync server* ("back up a server"): a server's folder into a
  folder of a share or of a removable drive;
- *to a cloud*: a folder of a share to a folder of a Google Drive or
  OneDrive account (below).

A server is reached through an **rsync daemon** (`rsync://user@host:873/
module/…`, a user and a password, unencrypted: the wizard says so) or
**over SSH** (host, port, user), the encrypted way: the NAS makes its own
key pair (ed25519, `ssh-keygen` in a unit of its own that may write only
`/var/lib/zendiloos-backup`, 0700) the first time it is asked for, and
shows the public key to put into the user's `authorized_keys`. The
server's host key is fetched (`ssh-keyscan` in a unit with the network,
as a dynamic user with no capability), shown with its fingerprint
(SHA256, as `ssh-keygen -lf` prints it) to compare with the server's own,
and pinned with the task; every connection checks it
(`StrictHostKeyChecking=yes` against a `known_hosts` of that line alone,
the system's ignored), and another key refuses the run with
`host_key_changed`. The wizard's "Check the connection" lists the
server's folder (`POST /backup/servers/check`). WebDAV, syncing between
devices and a backup client for computers are later.

**Modes, and how the target looks.** A task writes into one folder of its
target, named after the task when it was made (`folder`; renaming the
task keeps it), with a marker file `.zendiloos-backup`:

```text
<target>/<task>/.zendiloos-backup                 a backup of zendiloOS's
<target>/<task>/current/                          mirror: the copy
<target>/<task>/removed/2026-11-11_030000Z/       mirror, if kept: what that run deleted or replaced
<target>/<task>/2026-11-11_030000Z/               versioned: a complete version
<target>/<task>/2026-11-12_030000Z.incomplete/    versioned: a run under way, or cut off
```

- *Mirror*: an exact copy in `current/` (`--delete --delete-excluded`);
  optionally what a run deletes or replaces goes to `removed/<stamp>/`
  (`--backup --backup-dir`), kept for N days.
- *Versioned*: each run a dated version, files unchanged since the newest
  complete version hard links to it (`--link-dest`). Retention keeps the
  newest N and those younger than N days (either keeps a version; neither
  set keeps all; the newest is never removed), and runs after a run that
  completed, also removing the folders of runs cut off before.

A version is named after its run's start in UTC (`2026-11-11_030000Z`:
the names sort as the times do and none comes twice when the clock goes
back in autumn) and written as `<stamp>.incomplete`. On the NAS it takes
its name by one rename (`RENAME_NOREPLACE`) once rsync completed, then the
folder and the file system are written out (`fsync`, `syncfs`, so a drive
pulled out after "done" keeps it). A server cannot rename: there the
version is copied once more under its name as hard links of the
incomplete one (`--link-dest`, no data travels), and the incomplete one
removed. A run cut off therefore never leaves a version that looks
complete; its folder keeps the suffix, and the next run takes it over on
the NAS (renamed to its own stamp, so what it copied is not copied again)
or links from it on a server; retention removes what is left. A target
folder that holds anything but a backup of zendiloOS's (no marker, other
entries) is never written: a mirror would delete what is there. On a
drive whose file system has no hard links (FAT, exFAT, NTFS) only mirrors
are offered and refused otherwise, by zendiloosd and by the unit.

**Schedules.** Hourly at a minute, daily or weekly at a time, in the NAS's
time zone (the kernel's calendar, daylight saving time included, as the
volumes' backups plan); due once the latest time lies after the task was
made, changed or last started, so a time missed while the NAS was off is
made up once. "When this drive is plugged in" for a drive target: every
minute zendiloosd looks at the drives (which mounts a drive plugged in);
a drive mounted with another plugging in (`plugged_in`, as Storage tells
it: udev's `USEC_INITIALIZED` of its disk and the boot) than the one the
task last backed up for starts it once; a task made or changed while its
drive is there starts from that plugging in, and a restart of the NAS
counts as plugging in anew. A scheduled run to a drive that is not there
waits for the drive if the task backs up when it is plugged in, and is
logged and told as failed otherwise. One run per task at a time (in
zendiloosd, and systemd refuses a second unit of the name); a time that
comes while the task runs waits until it ended; while an update installs
nothing starts and what is due stays due (4.10). "Back up now" and "Stop"
(the unit stopped; the run logged as stopped, not told).

**A run** is a job (`backup.run`), with rsync's progress
(`--info=progress2`) as its percent, and logged: start, end, outcome
(succeeded; partial when rsync could not read some files or found them
gone, exit 23 or 24, the version complete but for them; failed; stopped;
interrupted by a restart of zendiloosd), files and bytes transferred
(`--info=stats2`), the version, the error; and its transfer list from
rsync's itemized changes, the first 5000 entries kept, the count of all.
Runs older than the log's days (90 unless set) are forgotten daily. A
failure is told (`backup_failed`, an error), a partial run as a warning
(`backup_partial`), a success when the task asks (`backup_succeeded`), in
the routing group *Backup* (4.3). The audit log has `backup.install`,
`backup.key`, `backup.task_create`, `backup.task_change`,
`backup.task_delete`, `backup.run`, `backup.stop`, `backup.restore`,
`backup.settings`, `backup.cloud_sign_in` and `backup.cloud_delete`.

**Filters and options.** Exclude patterns as rsync reads them (shown as
chips; `*.tmp`, `Thumbs.db`, `.DS_Store`, `@eaDir`, `.recycle`,
`#recycle` and `$RECYCLE.BIN` by default), checked to be one line without
the prefixes rsync reads as rules of their own; hidden files on or off
(`--exclude=.*`); uploads under way always left out. A bandwidth limit
(`--bwlimit`, KiB/s) as the settings' default or per task.

**Restore.** A task, then a version (or the mirror's copy), then all or
some of its folders and files (browsed one folder at a time,
`rsync --list-only`), to where the source was or into another folder of
a share (a server's backup always into a folder of the NAS), files that
are there kept (`--ignore-existing`) or replaced; never `--delete`. What
is restored into a share takes the folder's owner and the share's group
(`--chown`), and its modes and ACLs are not copied, so the share's
inherited ACL applies as to the explorer's copies (4.4); onto a drive as
its file system takes it. A restore is a job (`backup.restore`), one per
task at a time and not while the task runs.

**Sandbox: rsync never runs in zendiloosd or the agent.** The system agent
checks the places a request names (a share mounted at its root, a drive
mounted at its place; paths of the protocol's types: share paths, server
folders that never begin with `-`, host names and users that are no
options, patterns, version names) and runs the work in a transient unit
(`systemd-run --pipe`, this binary as `backup-unit`, one unit per task
and kind: `zendiloos-backup-run-<task>`, `-restore-`, `-prune-`; a
listing's of its own). The unit's sandbox: no new privileges,
`ProtectSystem=strict`, `ProtectHome`, its own `/tmp`, devices, IPC and
process view, `@system-service` system calls, 2 GiB of memory; of the
volumes and drives only the roots it works on (every `/volume<N>` and
`/run/zendiloos-removable` a read-only tmpfs, the source's root bound in
read-only, the target's writable); the system's secrets hidden
(`/etc/shadow`, `/etc/gshadow`, `/etc/ssh`, `/etc/samba`, `/etc/zendiloos`,
zendiloOS's and Samba's state, the backup's key folder, Docker's and
libvirt's state); the network only for a server (`PrivateNetwork=no`,
IP, Unix and netlink sockets), none otherwise; and the capabilities the
work needs: none for a server's listing or retention,
`CAP_DAC_READ_SEARCH` to read whatever a source holds, and to write on the
NAS as copied `CAP_CHOWN CAP_DAC_OVERRIDE CAP_DAC_READ_SEARCH CAP_FOWNER
CAP_FSETID CAP_SETFCAP`. Within, the folders are opened beneath their
roots by descriptor (`openat2` with `RESOLVE_BENEATH`, `NO_SYMLINKS`,
`NO_MAGICLINKS`, `NO_XDEV`; missing ones of a target made the same way)
and handed to rsync as `/proc/self/fd/<n>/`, so a folder swapped for a
link between the check and the copy changes nothing. rsync copies links
as links and never follows one; links that point outside the copied
folder, and every absolute link, are left out (`--safe-links`); it stays
on the source's file system (`--one-file-system`), skips devices and
special files, and pulling, a server's links get the same treatment on
the receiving side, so nothing is written outside the chosen folder
(beyond rsync 3.4's own checks of what a server sends). Should the client
hang up, the agent be asked to stop or the day run out, the agent stops
the unit first; units left running when the agent starts are stopped.
Runs and restores have four places of their own in the agent, so tasks
never keep the long places or the volumes' backups waiting; zendiloosd
waits a day and twenty minutes for them, the unit ten minutes less.

**Secrets.** A daemon's password is stored with its task in the database
(as the mail server's password, 4.3) and never sent back: the API says
only that one is set; left out of a change it stays for the same daemon,
port, user and module, and another server needs it typed again. It
travels zendiloosd → agent (the socket) → unit (its standard input, one
line of JSON) → a file in the unit's private `/tmp` (0600), which rsync
reads (`--password-file`); it is on no command line, in no unit property
and in no environment. The NAS's private key never leaves the NAS: root's
in `/var/lib/zendiloos-backup` (hidden in every unit), it reaches only the
run's unit as its credential (`LoadCredential=`, in
`$CREDENTIALS_DIRECTORY`, root's and the unit's alone). `ssh` runs with
`-F /dev/null`, `IdentitiesOnly`, `BatchMode`, no passwords and the pinned
`known_hosts` only.

**Clouds and encryption: rclone.** rsync does every task above; rclone
does a task to a cloud and every encrypted one, with the same modes,
layout, schedules, log, filters, restore and sandbox.

*Cloud accounts* are signed in to with OAuth's authorization code flow
without rclone on the administrator's computer: zendiloosd shows the
provider's consent page for rclone's own public OAuth client (or the
administrator's own client ID and secret, given per account), which
sends the browser on to `http://127.0.0.1:53682/` (Google) or
`http://localhost:53682/` (Microsoft), where nothing answers; the
administrator pastes that address back (`POST
/backup/accounts/sign-in/{flow}`, a sign-in kept 30 minutes, its state
checked), and zendiloosd exchanges the code for a token itself (https,
the token endpoints of Google and Microsoft) and keeps it, written as
rclone keeps one (`access_token`, `refresh_token`, `expiry`). Google
Drive is asked for `drive.file` only: rclone sees what it made, nothing
else of the drive. For OneDrive zendiloosd asks Microsoft Graph for the
account's drive (`/me/drive`: its id and type, which rclone needs). An
account stays while a task backs up into it (`account_in_use`, and the
database's foreign key); removing it forgets its token, not what was
backed up. rclone refreshes the token as it runs: the unit reads it back
from rclone's config file after every call of rclone, and zendiloosd keeps
the newest (also after listings and retention), so a refresh token that
rotates (Microsoft's) is never lost.

*Encryption* is rclone's crypt (names and contents, `filename_encryption
= standard`, folder names too) over the task's folder in its target, for
a task to a share, a drive, a cloud or an SSH server (rclone's SFTP, with
the NAS's key and the pinned host key as `known_hosts_file`); not for an
rsync daemon nor a pull. The administrator gives a password (8 characters
at least, typed twice) and optionally a salt (crypt's `password2`); both
are kept as the daemon's password is (below) and never sent back, and the
password cannot change while the task is encrypted, as what it backed up
is encrypted with it. The wizard says plainly that without them the
backup cannot be restored by anyone. Within the crypt the layout is the
one above, so the target shows the marker, `current`, `removed` and the
versions' names encrypted too, and nothing in plain text. A folder holding
entries the crypt cannot decrypt (another password, or other files) is
never written.

*Versions with rclone* are complete copies, as rclone has no hard links:
each takes as much space as the source, which the wizard says. In a cloud
the newest version is first copied on the provider's side
(`rclone copy`, server-side) into `<stamp>.incomplete`, so a run uploads
only what changed; on the NAS and over SFTP the run copies anew. rclone
then syncs the source into it (`rclone sync`, `--delete-excluded`) and
names it with one move (`rclone moveto`); a run cut off is taken over by
moving its folder. A mirror syncs into `current` with `--backup-dir
removed/<stamp>`. Retention removes with `rclone purge`, past the cloud's
bin (`use_trash = false`, `hard_delete = true`), where old versions would
take space on. The progress comes from rclone's statistics
(`--use-json-log --stats`), the transfer list from its log, written as
rsync's changes; a listing (`rclone lsjson`) is written as `rsync
--list-only` writes one, so zendiloosd reads both alike.

*rclone's sandbox* is the unit's above, with the network only for a cloud
or a server. Its remotes are set for rclone alone, by variables of its own
(`RCLONE_CONFIG_BASE_*` for the place, `RCLONE_CONFIG_DEST_*` for the
crypt), and a cloud's token by a config file in the unit's private `/tmp`
(0600); the crypt's password and salt are obscured as rclone keeps them by
`rclone obscure -` reading them on standard input. None of it is on a
command line or in a unit property. A folder of the NAS is opened beneath
its root as for rsync and given to rclone as `/proc/self/fd/<n>`; rclone
leaves links out (`--skip-links`) and never follows one, and it has no
`--one-file-system` of its own here (folders mounted within a share are
backed up with it). A restore from an encrypted or cloud backup copies
with rclone's filters for the folders asked for (`--filter-from`, their
names' glob characters escaped) and `--ignore-existing` to keep what is
there; what it wrote into a share then takes the folder's owner and group
(by descriptor, never through a link), as rsync's `--chown` does.

**Ownership, ACLs and attributes.** On the NAS (to a share or an ext4
drive) a backup keeps owners, groups, modes, hard links, ACLs and extended
attributes (`-rlptgoHAX --numeric-ids`). To a server it keeps modes,
times and hard links, and owners and groups where the server's user may
set them (as root, or rsync's fake-super); ACLs and extended attributes
stay on the NAS: rsync servers often lack the support or the rights, and
a restore gives files the share's ACL anyway. From a server: modes,
times, links within, hard links; its owners mean nothing here. On FAT,
exFAT or NTFS: files, folders and times (`--modify-window=1`). rclone
(clouds, encryption) keeps files, folders and times; owners, modes, ACLs,
attributes and hard links not.

**Shares and pools.** A target folder in a share stays in its share, with
the share's rules: whoever may read the share reads everything backed up
into it, whatever the source share's rules said; the wizard names who
that is (`GET /backup/shares/{id}/readers`, as the volumes' backups do),
and a target inside its source (or the other way round) is refused.

**The app** has an overview (how the backups stand, the last and next
run, the targets and whether they are there), the tasks as cards (name,
state, last run, source, target, mode, schedule; "Back up now" or "Stop",
and edit, check the connection, restore, log, delete), the restore, the
log (every run, each with its transfer list) and the settings (days the
log is kept, the default bandwidth limit, the cloud accounts signed in
to). A task is made in a wizard on
`StepDialog` (type, source and target, schedule and options, preview) and
changed in tabs of the same parts; folders come from the folder picker.
Deleting a task forgets it and its log; what it backed up stays.

**Tested** by unit tests of the domain (tasks, schedules, retention,
filters, paths, the pasted sign-in address), the agent's operations and
the unit's work (against real rsync where the machine has it: versions
with hard links, a run cut off taken over, a mirror's kept changes,
retention by name only, restores, links planted in the source; a fake
rsync for daemons and SSH; a fake rclone for clouds, the crypt, SFTP,
restores and retention, with its variables, config file and refreshed
token), the sign-in against fake token endpoints and Graph, feature tests
of the API, the frontend's tests, and in the second test VM (`just
vm-backup-test`, 8, an encrypted backup with real rclone among them).
Google Drive and OneDrive themselves are reached by none of these.

### 4.15 Configuration backup and recovery

**What is backed up.** One file, `zendiloos-config-<name>-<UTC
time>.tar.zst.age`: a tar archive, compressed with zstd and encrypted with
age's passphrase encryption (scrypt), all three made in zendiloosd itself
(the crates `tar`, `zstd` and `age`). Inside, in this order:
`manifest.json` (the layout, `1`, zendiloOS's version, the database's
schema, the machine's name, the time), `settings.json` (what the host
keeps outside zendiloOS's files: the time zone and time servers as
timedated has them, SSH's settings, the oldest TLS version),
`accounts.json` (the people's Linux users and groups with their UIDs,
GIDs and members, as the account files have them: the database keeps no
GIDs, and the files on the volumes carry the UIDs), `zendiloos.db` (a
consistent copy, `VACUUM INTO`, never the live file) and below `files/`
what zendiloOS wrote that its database does not say again, read by the
agent (`ReadConfiguration`, each a place of its own named by what it is,
never by a path, a plain file read without following a link): the
machine's own certificate and key, the issued certificates the web
interface serves, Samba's passwords (`passdb.tdb`: an account's SMB
password cannot be made from its web password's hash), ZFS's host id
(`/etc/hostid`, by which a pool knows its machine), the IDs handed out,
and for reference SSH's, timesyncd's and TLS's files and the network's
(`05-zendiloos-*`). Everything else follows from the database at the next
start, as always (1.2): Samba's and NFS's configuration, the web server's
ports and certificate, the firewall's rules.

**Encrypted, with a passphrase the administrator gives.** The archive
holds the password hashes, the certificates' keys, Samba's passwords and
the secrets kept for mail, backups and DNS, and it goes into a share,
onto a USB drive or into a browser's downloads. Written root's alone it
would be no safer there: Samba reads the shares as root, a drive goes
elsewhere. So it is encrypted, with a passphrase of at least 12
characters. *age* rather than `openssl enc`: age authenticates what it
decrypts (a wrong passphrase or a changed file is refused, never read as
garbage), its format is specified and small, and Debian's `age` opens a
backup by hand (`age -d … | zstd -d | tar -x`), where `openssl enc` has
no authenticated mode. age's command reads a passphrase only from a
terminal, so zendiloosd uses the library. Decrypting accepts a work
factor up to 2^22, so a backup a fast machine made opens on a slow one.

**By hand and on a schedule** (Control panel → Configuration backup,
`system.manage`): `POST /configuration/backup/download` answers the file
for the passphrase given (`configuration.download` in the audit log). The
plan (`GET` and `PUT /configuration/backup`, migration `0055`, one row)
backs up daily or weekly at a time of the NAS's time zone, as the
backup plugin's schedules reckon (made up once when the NAS was off), into
a folder of a share or of a removable drive (made if missing), and keeps
the newest N there (1 to 60, 7 by default; by name, which sorts as the
times do; other files are never touched). It needs neither the Backup
plugin nor rsync: zendiloosd writes the file through the files agent with
the access of the administrator who set the plan up (as the file manager
uploads: a hidden upload, then renamed), so the folder's rules hold; an
account that is gone or no longer an administrator fails the run. The
passphrase is kept with the plan as the mail server's password is (4.3):
never sent back, a change without one keeps it. `POST
/configuration/backup/run` backs up now. A run is a job
(`configuration.backup`); its outcome is kept with the plan, and a failed
one is told (`configuration_backup_failed`, an error of the group
*System*). Audited: `configuration.backup_settings`, `configuration.backup`.

**Restore** (`POST /configuration/restore`, `system.manage`; and `POST
/setup/restore` instead of creating the first administrator, allowed as
the setup is: the setup code, or an open setup from the own network,
tries throttled as the setup's). The body is the file, the passphrase in
`X-Zendiloos-Passphrase` as base64 of its UTF-8 (a header takes ASCII
only); nginx takes up to 257 MiB on these two paths, zendiloosd 256.
Refused while jobs or an update run (the restart would cut them off).
Checked first as a whole, before anything changes: the passphrase
(`wrong_passphrase`), the layout (only the entries above, each once,
plain files, the manifest first; `not_a_backup`, `unsupported_backup` for
another layout number), and the database: one of zendiloOS (its
migrations' table), not newer than this release knows (`backup_too_new`:
update first). An older one is taken as it is: the migrator brings it up
at the next start, a copy made before, as for an update. Then the host's
part goes back: the accounts' Linux users and groups with their IDs
(`RestoreAccounts`: where missing, refused as a whole if a name or ID is
another's here), the files (`RestoreConfiguration`, only those a backup
puts back, each checked as what it is: PEM, four bytes, `uid=`/`gid=`
lines; the certificates under the web server's lock, `nginx -t` and a
reload, the ones before back if nginx refuses; Samba stopped while its
passwords are written; the IDs handed out only rise), and the settings
through their own requests (the machine's name, 6; the clock; SSH; TLS),
each that fails only named in the answer. The database is staged as
`restore/zendiloos.db` with the restore's audit entry written into it
(the account by name: its ID is the other database's), and zendiloosd
exits two seconds after its answer (status 75), for systemd to start it
again (`Restart=on-failure`; run by hand, as `just dev` does, it is started
again by hand). At its start, before opening the database,
a staged one takes the place of the live one, which goes with its
write-ahead log to `backup/zendiloos-<time>-before-restore.db`. Whoever was
signed in signs in again, with the backup's accounts. Audited as
`configuration.restore`.

**What follows a restore at the next start.** Before the staged database
takes the live one's place, a mark is left beside it (`restore/follow-up`);
once the database is open and migrated, and before anything puts the
network's or the firewall's settings in force, zendiloosd sets the restored
database's network and firewall settings aside (they are the backed-up
machine's) and removes the mark. Done twice, setting aside changes nothing
more (only settings still kept are set aside), so a start cut off in between
simply does it again.

**The network: offered, interface by interface, with confirmation.**
Interfaces have other names on other hardware, and a fixed address from
elsewhere would cut the administrator off, so the restore leaves the
network as it is (the address the installer gave the fresh machine keeps
answering). The restored `network_setting_files` (migration `0054`) become
the backup's network settings (`network_restored`, `network_restored_files`,
migration `0059`), and this machine keeps none: the next look at the files
(section 6) takes them as they are, so nothing is told as drift. The Network
page lists the backup's interfaces and bridges, bonds and VLANs beside this
machine's (`GET /network/restored`): each *can be applied*, is *in force*
(the files say so already, whatever address DHCP asks for), *skipped* (no
interface of its name here, or not every member or parent of a device),
comes *with its device* (a member of the backup's bridge or bond), is *a
member here* or its name is *taken* by something else. Applying one (`POST
/network/restored/apply`, `system.manage`) is an ordinary change with the
confirmation of section 6: an interface's settings as `PUT
/network/interfaces/{name}` sets them (audited as `network.apply`), a
bridge, bond or VLAN made as `POST /network/devices` makes it
(`network.device_make`), whose own addresses follow as an interface of its
own once it is there. Never by itself. `DELETE /network/restored` forgets
them (`network.forget_restored`). The web interface's ports come from the
database: the answer says the HTTPS port, the interface moves there with
zendiloosd's start.

**The firewall: the defaults until the backup's rules are confirmed.**
Rules made for another network can shut the browser out, and at the start
nobody could confirm them. So the restored settings are set aside
(`firewall_restored`, `firewall_restored_rules`, migration `0060`) and the
machine starts with the firewall's defaults: off, everything allowed, as a
fresh installation has it. These are "the rules from before" for a restore
onto a machine installed afresh; on a machine restored from its control
panel, its own rules before went with the configuration the restore
replaced (they are in the database kept in `backup/`), and the defaults hold
there too until the backup's are confirmed. The Firewall tab shows the
backup's settings (`restored` in `GET /security/firewall`); applied (`POST
/security/firewall/restored/apply`, as any change: refused if they would
keep out the browser asking unless `lock_out`, audited as `firewall.apply`)
they are in force until confirmed within 90 seconds, otherwise the defaults
return by the agent's timer; confirmed, they are this machine's and offered
no more. `DELETE /security/firewall/restored` forgets them
(`firewall.forget_restored`).

**Pools of the old machine: imported when asked.** With the old data disks
in a machine installed afresh and the configuration restored, the database
names the pools (those its shares, installed apps, personal folders, the
virtual machines' storage and their disks are on, each with its volume
number), and the restored `/etc/hostid` makes them this machine's: ZFS
imports a pool without `-f` only if the machine that used it last has the
host id this one has now (or the pool was exported). They are not imported
by themselves: importing a pool another machine still uses would corrupt
it. At every start zendiloosd compares (section 5, *Pools of another
installation*): a pool the configuration names that is imported gets its
mounts made sure of; one that is not is told once (`pools_not_imported`, a
warning of the group *Storage*) and offered on the Storage page with what
`zpool import` says of it; one on no disk here is named as missing. The
Docker plugin, the VMs plugin, the Backup plugin and the ACME client are
installed again from the App Center; their settings are in the database.
Docker's data follows its pool: its mount comes back with the pool, and
installing Docker again onto that pool takes the dataset there as it is
(images, containers, volumes). The virtual machines' storage follows too:
its mount unit is what `vms_locations` reads, and libvirt's drop-in waits
for it again; their libvirt definitions, though, are not made anew from the
database (they are made when a VM is created or changed): after a restore
onto another system disk a VM has to be defined again, for now by changing
it once (a later step). The Backup plugin's SSH key and acme.sh's accounts
stay out (servers are given the new key; certificates are issued anew at
their renewal).

**Tested** by the domain's and use cases' tests (names, retention, the
schedule, passphrases, the plan, refusals, the restart), the agent's
against fake programs (the files read, links left out; certificates
nginx refuses going back; Samba stopped and started; accounts made with
their IDs and refused when taken), infra's (an archive written and read,
another layout refused, a backup restored into a fresh state directory
with its database staged, swapped in at the start and the audit entry in
it, a newer schema refused, backups written into a share through the
files agent, the mark of a restore to follow up; the network's and the
firewall's settings set aside once, this machine keeping none), the
network's and the firewall's use cases (the backup's interfaces beside
this machine's, applied and confirmed, refused where a name is missing;
the backup's rules in force only once confirmed, the defaults back
otherwise), the API's feature tests, the frontend's, `just
vm-config-backup-test` (VM 2 and a fresh VM 3), `just vm-pool-import-test`
(a pool of the old installation imported after a restore on its machine
installed afresh, its share reachable over SMB again and mounted after a
reboot; the backup's firewall rules applied with confirmation; a throwaway
VM 3) and `just iso-test`.

**Recovery mode (design; the rescue entry is built).** For a machine that
does not start, the ISO's menus have *Rescue a zendiloOS system*: Debian's
rescue mode (`rescue/enable=true`, none of zendiloOS's answers), a shell in
the installed system, from which a configuration backup can be copied
off (`/var/lib/zendiloos`), a database from `backup/` put back, or a
package repaired. The installer's environment has no ZFS, and a module
built for the installed kernel loads only into that kernel, so the pools
are not reached from there. Importing the pools of another installation
is built (above, and section 5): a fresh installation plus a configuration
backup is a whole recovery, the shares the database names come back on
their pools. What is to follow, in this order:

1. *A recovery boot entry of the installed system*: a GRUB entry
   `zendiloOS (recovery)` booting the installed kernel into
   `zendiloos-recovery.target`: no pools imported, no file services, no
   Docker or VMs; nginx and zendiloosd with an empty database of its own
   (`/run/zendiloos-recovery`), whose setup offers only a restore: a
   backup uploaded, or one found in `@recovery` folders, on removable
   drives and on pools imported read-only for the purpose (`zpool import
   -o readonly=on -N`, the newest per machine name listed). Restored, the
   machine starts normally.
2. *A live image* with ZFS for the ISO's kernel, built with the ISO
   (DKMS in the build container), for a machine whose system disk is gone:
   it imports pools read-only, finds backups as above and installs
   zendiloOS anew with the chosen backup restored at its first start
   (the installer's late command stages it in `/var/lib/zendiloos`).
3. *The VMs' libvirt definitions made anew from the database* where a
   VM's domain is missing, and the Backup plugin's SSH key, acme.sh's
   accounts and the projects' files in the archive.

### 4.16 UPS

**What it is.** A page of the control panel, *UPS* (under General), for
an uninterruptible power supply: where it is, what it says, when the NAS
shuts down on battery, and whether other devices may follow it through
the NAS. Seen with `system.view`, changed with `system.manage`. Its
context is `zendiloos-ups` (migration 0061: one row of settings as JSON,
and the password NUT's monitor on this machine signs in with, random,
made with the row and never shown).

**Network UPS Tools do the work.** zendiloOS writes NUT's configuration
and tells what the UPS's state does; watching the UPS and shutting the
machine down is NUT's own (`upsmon`), so a power cut shuts the NAS down
also while zendiloosd is stopped, being updated or busy. NUT comes from
Debian, installed on request by the package runner (4.10; task
`install-ups`: `nut-server nut-client nut-snmp` without recommended
packages), from the page itself, since a UPS is a setting and no app.
After apt, the runner asks udev to look at the USB devices again (`udevadm
trigger --subsystem-match=usb --action=change`): NUT's udev rules hand a
UPS on USB to the group `nut`, but count only for what is plugged in from
then on, and Debian's package triggers udev only where udev has an init
script, which Debian 13 no longer has. Without it a UPS that was plugged
in before NUT was installed would stay root's until pulled out and put
back, and the driver, which is not root, could not open it.

**Where the UPS is.** One UPS, in one of three places:

- *On this NAS, over USB*: one of NUT's USB drivers (`usbhid-ups`, which
  fits most UPSs of today, or `nutdrv_qx`, `blazer_usb`, `bcmxcp_usb`,
  `tripplite_usb`, `richcomm_usb`, `riello_usb`, `nutdrv_atcl_usb`), and
  optionally the vendor id, product id and serial number that name one
  UPS among several. *Detect* runs `nut-scanner -U` (in a transient unit:
  the agent's own sandbox has no USB devices) and offers what it found; a
  single UPS found is taken.
- *On the network, over SNMP* (`snmp-ups`): the address of the UPS's
  network card, a community, v1 or v2c.
- *On another device* that runs NUT's server (another NAS, a server): its
  address and port, the UPS's name there, an account and its password.
  This NAS then runs no driver and no server, only the monitor, as a
  secondary.

A fourth, *simulated* (NUT's `dummy-ups`, played from
`/etc/nut/zendiloos-simulated.dev`), can be set through the API only and
is what the VM test follows; its account may set the UPS's variables
(`upsrw`), which is how a test plays a power cut.

**The files.** The agent (`SetUps`, with the settings checked again as it
parses them: every word that goes into a file is one of a narrow set of
characters, so nothing can add a line or a section) writes NUT's own
files in `/etc/nut` whole, readable by root and the folder's group (`nut`)
as the package has them, since they hold passwords: `nut.conf` (the
mode: `standalone`, `netserver` when other devices follow, `netclient`
for another device's UPS, `none` without a UPS), `ups.conf` (the driver,
always in a section called `ups`), `upsd.conf` (`LISTEN 127.0.0.1 3493`,
or `0.0.0.0` when other devices follow), `upsd.users` (the monitor's own
account `upsmon_local` as primary; the account for other devices as
secondary), `upsmon.conf` (the `MONITOR` line, `SHUTDOWNCMD
"/sbin/shutdown -h +0"`) and `upssched.conf` with its script
`zendiloos-timer`. What stood in a file before zendiloOS wrote it the
first time (Debian's sample, or a configuration made by hand) is kept
beside it as `<name>.before-zendiloos`. Debian's
`nut-driver-enumerator` then makes the unit `nut-driver@ups` from
`ups.conf`, and the agent enables and restarts `nut-server` and
`nut-monitor` (for another device's UPS: disables the server). A driver
whose section changed is started anew by the agent, not left to the
enumerator: that of NUT 2.8.1 settles for the driver's "live reload",
which reports that it sufficed and leaves `ignorelb` as it was, so that a
charge to shut down at would not be in force until the next restart
(found with the simulated UPS: at 30 % with 40 % set, no low battery was
reported). Neither the
enumerator nor the driver is waited for (`--no-block`): the enumerator
can wait a minute and a half for a driver, and a driver whose UPS is not
plugged in, or does not answer on the network, takes that long to give
up, which had a change run into nginx's timeout. A driver that does not
start does not make the settings fail; systemd goes on trying it, the
server takes it once it is there, and the page says meanwhile that the
UPS does not answer.

Settings that are in force already change nothing and interrupt nothing:
no file is written and no unit restarted when the files say what they
would say and NUT runs. zendiloosd asks for the settings it keeps at
every start (1.2), so that the machine follows a configuration that was
restored (4.15, the settings are in the database); restored on a fresh
installation, they wait until NUT is installed, which the page offers and
whose job puts them in force. Without a UPS in the settings, NUT is
switched off (`MODE=none`, server and monitor disabled and stopped) only
where zendiloOS set it up, which `nut.conf` says in its first line: a NUT
an administrator configured by hand on the Debian below, and never
through zendiloOS, is left as it is.

**When the NAS shuts down.** On battery, one of three:

- *when the UPS reports its battery as low*: `upsmon`'s own rule (`OB LB`);
- *when the charge falls below a percentage* (5 to 95): the driver's doing
  (`ignorelb` and `override.battery.charge.low`), which then calls the
  battery low at that charge whatever the UPS itself thinks low; only for
  a UPS whose driver runs here;
- *after a number of minutes on battery* (1 to 720): `upsmon` runs
  `upssched` when the power fails and returns, whose timer, dropped when
  the power is back in time, ends in `upsmon -c fsd`, a forced shutdown.
  The low battery still shuts the NAS down earlier.

`upsmon` tells the devices that follow before this machine goes down
(`FSD`, and it waits `HOSTSYNC` 15 seconds for them). *Cut the output
afterwards* (`POWERDOWNFLAG /etc/killpower`) has Debian's shutdown hook
(`nutshutdown`) tell the UPS at the very end to switch its output off; it
switches on again when the power returns, and a machine whose firmware
starts it when power is applied then starts by itself. Only a UPS whose
driver runs here can be told. There is no standby mode: a NAS on battery
either keeps running or shuts down.

**Other devices.** With *pass the state on*, NUT's server listens on the
network and has an account for the devices that follow (`monuser` unless
another is named; a Synology NAS cannot be given an account and signs in
as `monuser` with the password `secret`). The page says what to enter
there: the NAS's address, port 3493, the UPS's name `ups`, the account.
NUT's protocol is not encrypted here: the account's password crosses the
LAN in the clear, as between any two NAS that follow one UPS, and the
account may only follow, never set anything. The firewall (4.12) has no
service of that name yet: where it is on, a rule for TCP port 3493 lets
those devices in, which the page says.

**Its state and what is told.** zendiloosd reads the UPS's variables
itself (3.4), for the page (looked at again every ten seconds while it is
open) and, every ten seconds, for the notifications: a power cut
(`ups_on_battery`, with the rule the NAS shuts down by and what the
battery holds), its end (`ups_on_mains`), the shutdown (`ups_shutdown`:
the battery is low, or the time on battery is over), a battery that needs
replacing (`ups_replace_battery`) and a UPS that has not answered three
times in a row (`ups_unreachable`: a power cut would go unnoticed), each
once per change and each a kind of its own in the channels' table (group
System). On battery zendiloosd looks every three seconds, and `upsmon`
waits ten (`FINALDELAY`) between deciding to shut down and doing it, so
that the shutdown is told before zendiloosd is stopped; whether a mail
still gets out then is the network's to say. A NAS that follows another
device's UPS and shuts down after its minutes on battery does not tell the
shutdown, as no state of the UPS shows it: the power cut's notification
said when it would come.
The first look after a start, or after the settings changed, tells
nothing but trouble that is there already (a NAS that starts on battery
says so); a power cut that ended while the NAS was down is not told as
over. Passwords are
kept in the database as they are, since NUT needs them in its files, and
never leave it through the API (`password_set`); one left out on a change
stays the one kept, for the same account (and host).

**The API.** `GET /api/v1/ups` (installed or not, the SNMP driver, the
settings, the reading, the drivers offered), `PUT /api/v1/ups` (put in
force first, kept only then; what NUT's side refused is not kept; audited
as `ups.update`), `POST /api/v1/ups/install` (the job `ups.install`,
audited) and `POST /api/v1/ups/scan`.

**Not there.** Several UPSs; SNMP v3; NUT's protocol over TLS; a
firewall service for port 3493; the UPS's own commands (a battery test,
the beeper). Telling devices that speak no NUT.

---

## 5. Storage Concept

**Decided: ZFS is the default** (2026-11-05). One mechanism provides
redundancy, checksums with self-healing, snapshots, datasets and scrub, so
the interface and the agent deal with a single tool chain (`zpool`, `zfs`).

| Term | Implementation |
|---|---|
| Disk | detected with `lsblk --json`, identified by serial number and `/dev/disk/by-id`, never by `sdX` |
| Disk preparation | GPT with one partition of the type OpenZFS uses, from 1 MiB to 128 MiB before the end; the reserve lets a slightly smaller disk of the same nominal size replace a member |
| Pool | one zpool with one vdev: single disk, mirror (2 or more disks), RAIDZ1 (3+), RAIDZ2 (4+), RAIDZ3 (5+); `ashift=12`; members referenced by `/dev/disk/by-id/<disk>-part1` |
| Volume | the pool's root dataset, mounted at **`/volume<N>`** (the lowest free number). One pool is one volume |
| Mounting | `mountpoint=legacy` and a systemd mount unit per volume (`volume1.mount`, after `zfs-import.target`): mounts happen in the host's namespace rather than the hardened agent's, and the boot order is explicit |
| Pools of another installation | the pools the configuration names (shares, installed apps, the personal folders, the virtual machines' storage and disks are on them, each with its volume number: `KnownPools` in `zendiloos-infra`) against those imported and those `zpool import -d /dev/disk/by-id` finds on the disks (the agent's `ImportablePools`, its text parsed in zendiloosd: name, numeric identifier, state, ZFS's status and action, the devices; *last used by another machine* where ZFS asks for `-f`, *cannot be imported* where it says so). **At every start** (`PoolImports::reconcile`, once the agent answers) the mounts of each pool it names that is imported are made sure of (`MountVolume`: the volume's, its shares' whose datasets are there, the personal folders', Docker's and the VMs' datasets' where the pool has them, each unit written only when its content differs from what creating it writes, all enabled with `--no-reload`, systemd reloaded once if any changed or `NeedDaemonReload` says so, then started, the volume's first; Docker's and libvirt's drop-ins as enabling them writes them; Docker started once its data is mounted anew; one that fails to start named, the others started), and Samba and NFS follow once a volume's mounts were made anew (`VolumeObserver`, the sharing configuration written again, so NFS exports the datasets rather than the folders beneath). A pool that says another volume number than the configuration is left alone; those not imported are told once a start (`pools_not_imported`). **Importing is asked for, never done by itself** (Storage → Pools, `GET /storage/pools/absent`, which the page asks for once and after storage jobs, as `zpool import` reads every disk's labels and so wakes a disk asleep; `POST /storage/pools/import`, `storage.manage`, a job `storage.import_pool`, audited): a pool the configuration names as its volume, with its shares; another one *adopted* as the volume asked for or the lowest free (none an imported pool or the configuration has: `volume_taken`), only if zendiloOS made it (`zendiloos:volume` on its root dataset, which then says the new number; otherwise it is exported again); its folders are there, no share of this configuration points to them. The agent imports by the identifier listed, which must be the name's, unmounted (`zpool import -N -d /dev/disk/by-id <id>`, so its members are named as creating names them, and the cache file has it for the next boot), refuses a name imported or a volume number taken (also by a unit for another pool), and exports again what is not the volume it was asked for. ZFS refuses a pool another machine used last and did not export, and so does zendiloOS: only `force` with the pool's name typed imports it with `-f` (`other_host`); the page says why (a pool two machines use at once is damaged beyond repair) and that a configuration backup restored brings back the host id, so a pool of the machine backed up is this machine's own. A new pool never takes the volume number of a pool the configuration names that is not imported. Tested in the agent against fake programs, by the use cases', the API's and the frontend's tests, and in a throwaway VM reinstalled on its spare disks (`just vm-pool-import-test`) |
| Dataset defaults | `compression=lz4`, `atime=off`, `xattr=sa`, `acltype=posixacl` (for the shares' one ACL), `dnodesize=auto`, `setuid=off`, `devices=off`; `normalization=formD`, `utf8only=on` (fixed at creation) |
| Share | a child dataset of the volume, mounted at `/volume<N>/<share>` by its own unit (`volume1-photos.mount`); created with `setuid=off` and `devices=off` also on pools made before those were defaults. Per share, stored in the database and set at every change and at startup: a quota on its own data (`refquota`, snapshots not counted, at least 1 GiB, never below what it holds), compression (`lz4`, `zstd` or off) and record size (16 KiB, 128 KiB, 1 MiB); what it holds comes live from ZFS |
| Snapshots | per share (child dataset), by hand or on schedules (hourly, daily, weekly, monthly, each keeping 1 to 100). Named after their UTC time, `zdl-YYYY-MM-DD_HH.MM.SS`, the form Samba's `vfs_shadow_copy2` reads, so Windows offers every snapshot as "Previous Versions" (`shadow:snapdir = .zfs/snapshot`); the schedules a snapshot was taken for and a note are ZFS user properties (`zendiloos:schedule`, `zendiloos:note`), the schedules themselves live in the database. `zendiloosd` looks every five minutes: a schedule that never took one, or missed its time while the machine was off, takes one at once; when several are due, one snapshot counts for all of them. Each schedule keeps its newest; a snapshot goes only when every schedule it was taken for lets it go, never one taken by hand or of a removed schedule. A failing schedule becomes one notification until it works again. The agent takes and destroys snapshots only of a share dataset on a pool zendiloos created, and the label's shape rules out ZFS's ranges and lists. The file manager looks into a snapshot as it was: the files agent opens `.zfs/snapshot/<label>` of the mounted share as a root of its own (the label's shape checked; opening it for reading makes ZFS mount it, which `RESOLVE_NO_XDEV` from the share would refuse), read only, and refuses every change in it; folders list, files download and preview, folders download as archives from it, and entries are copied back into the share, into the folder they were in and beside a name that is taken, so nothing is replaced (a job). A share can be rolled back to one of its snapshots (`zfs rollback -r`, the agent only on its own share datasets): what changed since is lost and so are the later snapshots, which the interface lists, so the share's name must be typed to confirm; the files get the permissions recorded now again, not those of the snapshot (4.4) |
| Monitoring | `zpool status` (health, resilver and scrub progress, errors per device) every minute, SMART of disks holding data every six hours; changes become notifications (degraded or recovered pool, finished resilver or scrub, volume 80 or 90 % full, a disk's SMART getting worse); scrub monthly (second Sunday) by the cron job of `zfsutils-linux`, and on demand; a running scrub can be stopped (`zpool scrub -s`), a resilver cannot; while a scan runs, its speed and the time left are estimated as `zpool status` does (bytes issued in the current pass over its time minus pauses); the package depends on a cron daemon, which Debian's cloud image lacks. The concerns, when there are any, are listed above the Pools and Disks pages under one verdict, the worst first: pool unavailable or degraded, unrepairable files, device errors, volume 80 or 90 % full, a disk's SMART verdict, no redundancy, no check for 40 days; a disk is named there as the pages name it, by its kernel name with its slot (where the enclosure places it) and its model or serial number, also a device of a pool, whose `/dev/disk/by-id` or `by-path` link (`wwn-…`, `ata-…_<serial>`, with or without `-part<N>`) or kernel name ZFS reports is matched to the disk it is on, by the link lsblk reports, the WWN, a serial number only one disk reports or the port; one no disk matches keeps the name ZFS gives it, which the concern keeps besides (`device`), and a disk's SMART notification names its slot too; SMART verdicts are the monitor's last, so asking reads no disk; an eMMC device (a system disk on many small boards), which has no SMART, reports in its place what eMMC 5.0 and later keep in EXT_CSD: two estimates of the life used, in steps of ten percent, for its two kinds of memory, and how far its reserved blocks are used (normal, 80 %, 90 %); 80 % of its life or more, or its reserve at 80 %, is a warning (earlier than NVMe's 90 %, since eMMC is soldered to the board and the system has to move first), past its life or its reserve at 90 % failing, and it is told as a disk's SMART is; an older eMMC and an SD card report nothing; a disk can test itself (short or long SMART self-test, `smartctl --test`), its report shows the progress and the drive's own estimate of how long each test takes, and a running test can be stopped; a disk that sleeps, reports no SMART or cannot test itself is refused rather than woken. Self-tests also run on a schedule: one plan per kind (the long test weekly or monthly), the disks by serial number or all that can, starting from a local hour (the host's time zone, read with `jiff`) within four hours, so a NAS switched on late still tests the same night; the long test goes first, a disk already testing is left alone, a sleeping one is woken because the plan asked for it, and a failed test reaches the owner through the monitor's SMART notifications |
| Existing storage | mdadm arrays found on the system are shown read-only on the Pools page; other filesystems (Btrfs, ext4, XFS) only through `GET /storage/volumes`. zendiloos does not create or change them |
| Removable drives | USB sticks and disks, memory cards, for administrators only (`storage.manage`), never shared: a file system that passes the checks of 4.7 is named by its UUID and mounted at **`/run/zendiloos-removable/<id>`**, root's, outside the volumes (so no share, Samba or NFS export reaches it) and on a tmpfs (so nothing of it outlives a restart). `nosuid,nodev,noexec,noatime`; FAT (`utf8,shortname=mixed,flush`), exFAT and NTFS (the kernel's `ntfs3`, `windows_names`) have no owners of their own, so everything on them belongs to root and the group `zendiloos-users` (`uid=0,gid=…,fmask=0117,dmask=0007`): readable and writable by the accounts the files agent acts as, by nobody else, executable by nobody; ext4 keeps its owners and permissions, which the files agent overrides there with root's access to files, on threads of their own (4.4), so a drive formatted elsewhere is fully usable; what it writes there belongs to root. A drive that cannot be written (write-protected, NTFS that Windows left hibernated) is mounted read-only. Mounting happens in the host's namespace: the agent asks systemd for a transient mount unit (`systemd-mount --collect --fsck=no`), gone at the next boot and bound to its device, so systemd unmounts it when the drive is pulled out; `umount`, removing the place (`rmdir`) and powering off run in transient services (`systemd-run`), outside the agent's sandbox, which keeps `/run` read-only and only sees the mounts. `zendiloosd` mounts a drive when it sees it, as Windows does: whenever the drives are listed (the file explorer asks while it is open) and once a minute besides; the agent and `zendiloosd` speak `AF_UNIX` only, so no udev or netlink hook is needed. Ejecting unmounts (refused while files on it are open, `drive_in_use`), removes the place and, by default, takes the disk off the bus (this binary as `power-off-drive <disk>`: the SCSI device is deleted, which writes its cache out and stops it, then its USB device is removed, which powers the port off where the hub can), so it can be pulled out; without powering off, the drive stays plugged in and unmounted until it is pulled out or mounted by hand, e.g. to make a pool of it. That it was ejected is kept in the database (`removable_ejected`, migration `0047`) with the plugging in of its disk, the boot's id and when udev set the disk up (`USEC_INITIALIZED`, which it sets anew whenever the disk is added and keeps through its change events; the kernel's `DISKSEQ` is no such mark, since a removable disk gets another at every media change the kernel assumes, as when another disk is plugged into the same bus), so a restart of `zendiloosd` or the agent does not mount it again; it holds until the drive is pulled out or seen plugged in anew, also while nobody looked, and a restart of the NAS counts as plugging in anew. Until it is known which drives were ejected, none is mounted by itself. A drive pulled out while mounted becomes a notification (`removable_pulled`), and what is left of it is let go of lazily (`umount --lazy` once its device is gone) and its place removed; so are empty places from before a restart. The API: `GET /storage/removable` (id, label, file system, device and disk, model, size, state `mounted`, `ejected`, `failed` with the agent's reason, or `unmounted`, `read_only`, `path`, space while mounted), `POST /storage/removable/{id}/mount` and `…/eject` (`power_off`, true by default). The simulation offers a USB stick (`1A2B-3C4D`, FAT) |
| Enclosure | where each disk sits: drawn above the list of disks, as a schematic in the interface's own colours (no vendor pictures), each slot in the colour of its disk's health. An enclosure is up to 16 groups of slots, each with rows and columns (60 slots at most), bays upright side by side (a tower's front) or lying stacked (a rack, a cage of 2.5" bays), a form (3.5", 2.5", M.2) and a numbering direction; a slot has a label (up to 32 characters, names up to 64) and is bound to a port or a serial number. A port is udev's `ID_PATH` of the whole disk, its `/dev/disk/by-path` name without `-partN` (`pci-0000:05:00.0-ata-1`; `…-ata-1` and `…-ata-1.0`, as systemd 253 and newer write it, are the same port), so a disk replaced in that slot shows up there by itself; a serial number serves ports that are not stable (USB, some HBAs). Disks are placed by port first, then by serial number; a slot without a disk is empty, one whose disk bound by serial number is not there is missing, and the disks in no slot are listed as not placed. A serial number two disks present report (the disks of some USB docks report their bridge's) names neither, so one never takes the other's slot: those are bound by port, and the inventory and the monitor keep them apart as `SERIAL@ID_PATH`. Such a serial number is noted as shared the first time two disks report it at once (migration `0046_disk_shared_serials`, which also learnt the ones the inventory had kept apart before and dropped their records by the serial number alone where nothing was noted on them), and from then on its disks keep that key also while one of them is there alone, so a disk's record and verdict never change with which others are present. Placement comes at once: the slots show the SMART verdicts the monitor has read so far and never wait for a pass of it reading the disks, the rest follows when read. A catalog of NAS models is built in: a TOML file each (`zendiloos-infra/src/enclosure/`), embedded in the binary and checked when it is read, each recognised by DMI's `sys_vendor` and `product_name` (trimmed, whatever the case), with its case for the drawing (tower, rack or plain; a front panel on the left or right with its power button and lights), its slots bound to ports, whether someone checked it on the real machine, and notes. The first is the UGREEN DXP4800 Plus, from the owner's machine running UGOS: four 3.5" bays at the front (`pci-0000:05:00.0-ata-1` to `-4`, agreeing with miskcoo/ugreen_leds_controller's ataN to diskN), two M.2 slots underneath (`0000:01:00.0`, `0000:02:00.0`) and the system SSD inside (`0000:04:00.0`); the order of bays 1–4 and which slot is printed "M.2 1" are still to be confirmed. While a model is not checked on a real machine, every place that names one of its slots says that its bay order is not confirmed yet: the one-click offer, the schematic, a disk's slot and the bay of a disk being replaced; a slot bound by hand is named as it is. Kept in the database (migration `0036_enclosure`): a model with other bindings for some of its slots, or a layout of one's own, which may start from a model; while nothing is chosen, the model the machine was recognised as is offered in one click; a machine recognised as none starts the setup with a layout of its own, not with a model of the catalog. A slot of a model is bound otherwise once at most (else `invalid_input`). A choice saved is answered as saved also while the disks cannot be read back, with none placed. **Locating** a disk makes its activity light blink so it can be found: zendiloosd has the agent read from it in bursts of 0.7 seconds with pauses of 0.7 seconds (4.6), for 60 seconds or up to 300, one session per disk, which can be stopped, a burst under way too; a burst that takes longer than 5 seconds (a disk or the agent hanging) ends the session; until when it blinks is in the API. NVMe M.2 slots mostly have no light of their own. **[Later]** the enclosure's own lights: `ledctl` (ledmon) for backplanes that have them, UGREEN's front lights over I²C. The **inventory** keeps every disk seen, by serial number: model and size as last seen, first and last seen (the last up to an hour behind, so a look every minute does not write every minute, and never going back, not for a clock set back either), and what the owner notes: the purchase date, until when the warranty holds, the vendor or shop, a note. A disk no longer present stays listed as stored until it is removed. Reading needs `storage.view`, changing `storage.manage`; changes are audited (`storage.enclosure_update`, `storage.locate`, `storage.locate_stop`, `storage.inventory_update`, `storage.inventory_remove`), those refused (no such disk kept, the disk still present) or failed as failures. Noting details is answered as kept also while the disks cannot be read, without whether the disk is present. The API: `GET`, `PUT` and `DELETE /storage/enclosure`, `GET /storage/enclosure/models`, `GET /storage/locate`, `POST` and `DELETE /storage/disks/{name}/locate`, `GET /storage/inventory`, `PUT` and `DELETE /storage/inventory/{serial}`; disks carry `id_path`. The simulation is a DXP4800 Plus with its disks in the bays; tested in the VM with `just vm-enclosure-test` |
| Kernel module | `zfs-dkms` and `zfsutils-linux` from Debian contrib, built for the installed kernel with its headers |

| Option | Pros | Cons |
|---|---|---|
| **ZFS** (chosen) | self-healing with checksums and redundancy in one layer, snapshots, datasets, send/receive, mature | out-of-tree module (DKMS) that must be rebuilt for every kernel and may lag behind a new kernel; licensing questions for prebuilt modules in an image (see section 7) |
| mdadm + Btrfs | in-kernel, extremely well tested, all RAID levels | Btrfs detects bit rot on top of mdadm but cannot repair it; two layers to manage |
| Btrfs native RAID | self-healing, in-kernel | RAID 5/6 has known problems (write hole) |

---

## 6. Further Decisions

| Area | Decision | Alternatives / rationale |
|---|---|---|
| Base | Debian 13 "trixie" (kernel 6.12 LTS), amd64 and arm64 | current stable; backports kernel for new hardware if needed |
| Target hardware | x86-64 NAS appliances with Intel CPUs and generic x86-64 systems with UEFI (self-built, mini PCs); arm64 NAS boards and single-board computers with SATA or NVMe whose firmware starts Debian's installer through UEFI (EDK2, or U-Boot's UEFI) and whose hardware Debian's own kernel supports | phase 5: boot medium, LEDs and fans (often via I²C), Intel GPU drivers; boards that need a kernel or device tree of their vendor are not covered |
| Development environment | tools installed on the development machine (README, "Development"): Rust from `rust-toolchain.toml` through rustup, Node from `.nvmrc`; CI installs the same versions | packages and images are built in a `debian:trixie` container |
| Database | SQLite (WAL) with sqlx; compile-time checked queries (`query!`), offline data in `backend/.sqlx` | after changing SQL or migrations run `just sqlx-prepare`; `just check` fails if the data is stale |
| Writes to the system disk | measured with zendiloosd idle on the simulated NAS under `strace` (2026-11-11), the agent's share from the requests it was sent: the database got one transaction (12 pages of the write-ahead log) in 3.5 minutes and no `fsync` (WAL with `synchronous=NORMAL` syncs at checkpoints); its regular writes are already spaced (a session's last use at most once a minute, a disk's last sighting once an hour, an unchanged upsert writes nothing). Metrics and the containers' use over time are kept in memory only, Docker's data and its containers' logs on a pool; zendiloosd traces HTTP requests at debug, and nginx's access log gets the API's requests only while a page is open and polls (every 5 to 30 seconds), which is left as it is. What was wasteful went into the journal: the agent logged every request it carried out at info, some 20 a minute on an idle NAS (the disks and pools every minute, what the firewall needs to know every 30 seconds, what containers use every 20), about 29 000 lines and, at some 0.6 KB an entry, 15 to 20 MB a day; and each reload of systemd (3.3) wrote its lines and, on eMMC, the GPT generator's errors. Requests that only read (`Request::only_reads`) are now logged at debug when they go well; reloads happen only for changed units, the generator is masked (1.3), and the journal keeps at most 512 MiB (1.3) | eMMC 5.0 and later report their wear (5, Monitoring); the database's writes needed no batching |
| Migrations | forward only, embedded, transactional; before each migration a copy of the database in `/var/lib/zendiloos/backup/` | safe migrations without down scripts |
| Configuration | static: `/etc/zendiloos/*.toml` (listen address, paths, log level, development flags); everything the admin manages lives in the database | |
| System metrics | `sysinfo` for CPU, memory, processes, sensors; an own parser for `/proc/diskstats` | sysinfo does not cover storage specifics |
| Network | systemd-networkd + systemd-resolved; NetworkManager is only shown. Per interface: IPv4 by DHCP, fixed (address, prefix, gateway) or off, IPv6 from router advertisements or off (both off: the interface switched off, `ActivationPolicy=down`), up to three name servers; an interface without a file of zendiloOS's (down, or never set up) is switched on the same way. The agent writes `/etc/systemd/network/05-zendiloos-<interface>.network` (before netplan's files), after a record of every file a change touches as it was (`zendiloos-change.json` beside them, root's alone), and has systemd-networkd reload and reconfigure the link over D-Bus (`busctl`: the agent's own network namespace has neither the host's links nor a working `networkctl`). DHCP names the card by its MAC address (`ClientIdentifier=mac`): systemd's default, a DUID from the machine ID, changes with every installation, and so do those of ifupdown's DHCP clients (dhcpcd's `duid`, dhclient's `-i`); and it asks for the address the card has (`RequestAddress=`), so a server that keyed the lease by another client's identifier hands it out again once that one released it. A change is kept only if confirmed within two minutes, from the new address if it changed; otherwise every file returns as the record says, from wherever the change got to, by a timer in the agent, and at the agent's start should it or the machine restart first; the record goes last, so an undoing cut off is done again (a `….previous` file an earlier agent kept beside a file returns at the start too). One change at a time. **zendiloOS's settings are kept in its database** (migration `0054_network_settings`): its files as they were once a change was confirmed (an interface's, a device's, a takeover's, a restore's), so they are no longer the only record; the first start with that table keeps the files as they are. At each start, once the agent answers and no change waits (one the agent's own start undoes, say), zendiloosd compares the files with the settings, both read by the same code, so only what zendiloOS's settings say counts (addresses, DHCP, name servers, ports, VLANs, a device's kind, members, mode, STP and MAC address), not how a file is written. A file lost, one that says otherwise, or one of zendiloOS's that its settings do not have is told as a notification (`network_drift`, each difference once, also across restarts) and on the Network page (`drift` in `GET /network/setup`, live while no change waits), which offers two ways out: **restore** (`POST /network/restore`, audited as `network.restore`) writes the settings back as one change with confirmation like any other, every file lost or changed as kept, a device the settings do not have removed, an interface's file they do not have taken away (`unset` in the agent's change), and unconfirmed within two minutes the files are as they were; **keep the files** (`POST /network/keep-files`, `network.keep_files`) takes them as the settings, for a file changed by hand on purpose. Until one of them is chosen, no other change is made (`network_drift`, 409): it would be planned on files zendiloOS did not write, and confirming it would keep them. After a configuration backup is restored, the settings its database brought are the backed-up machine's: they are set aside before the first look (migration `0059`), so the files here are kept as they are rather than told as drift, and the Network page offers the backup's interfaces and devices one by one, each applied as an ordinary change with confirmation where this machine has its names (4.15). zendiloosd never writes the files back by itself at the start. The NAS runs as the files say, and a file changed outside zendiloOS may be what keeps it reachable now (an address set at the console, a card renamed by new hardware); written back at boot, a working network could be cut off with nobody there to confirm, and the confirmation that guards every change would only take the network away for two minutes before giving the changed files back. Where the change made the NAS unreachable, nothing zendiloosd does unattended helps either: what helps is the console, or restoring from the page once it is reachable. **Bridges, bonds and VLANs** are made, changed and removed on the Network page (`POST /network/devices`, `PUT` and `DELETE /network/devices/{name}`, `system.manage`, audited as `network.device_make`, `network.device_change`, `network.device_remove`), each as one such change of several files: a `05-zendiloos-<name>.netdev` for the device and the `.network` files of the device, its members and its parent. A bridge has wired cards, and bonds and VLANs made here, as members (or none, for VMs only), STP off unless asked; a bond has wired cards (active-backup, 802.3ad/LACP or balance-alb; its members' links watched every 100 ms, LACP asked to tell every second and traffic spread by address and port); a VLAN is on a wired card, a bond or a bridge with an id from 1 to 4094. Members are free: in no other bridge or bond (by zendiloOS's files or the kernel's), carrying no VLAN; names are interface names the kernel takes (15 characters at most), none in use, none Docker's (`docker0`, `br-` and twelve hex digits) or a `veth`'s, which the firewall takes for containers'; nothing may end up on itself (a VLAN on a bridge among its own members), all checked before anything is written, and `POST /network/devices/plan` says beforehand what becomes of each interface. A bridge or bond takes the settings of the first member given that has any, which becomes its first member, and that member's MAC address (`MACAddress=` in the `.netdev`), so DHCP, by the MAC address and asking for the address the card had, keeps the NAS's address; the other members' settings go. Its members are noted in order in its `.netdev` (`# Members:`); removed, it gives its settings back to its first member, asking for the address it has now, and the other members are switched off, as are members taken out of one. A VLAN has settings of its own, none by default: a VLAN made never brings a second default route or other name servers that could take the NAS's confirmation away, and one carrying VMs needs no address; its parent keeps its settings and carries it (`VLAN=`). Devices zendiloOS made stay up without addresses (`RequiredForOnline=no`) rather than being switched off. systemd-networkd makes a new `.netdev`'s device when it reloads (a VLAN's when its parent is set up) but neither removes nor changes one it made: a device that goes, or whose bond mode or STP changes, is deleted in a transient unit (`ip link delete`, 3.3) and, if it stays, made anew from its file; undoing a change deletes the devices it made and has systemd-networkd make those it removed again. A device that carries a VLAN or is a member of another is removed last; a bridge a VM is on is not removed, nor is a card or bond a VM is directly on taken into a bridge or bond (`device_in_use`, 4.13). A bridge made without members and without an address of the NAS's is an internal network for VMs, which the NAS keeps itself off by a table in nftables' `bridge` family (4.12); `GET /network/setup` names them (`internal`). An interface's settings are changed as before; a member's are its device's, and a parent keeps its VLANs. Firewall rules that name a member see its traffic no longer: it comes in on the device. Bridges, bonds and VLANs not made by zendiloOS are shown with their members and parent and their IP settings changed, but neither changed otherwise nor removed. **The installation ISO** hands the network to systemd-networkd and systemd-resolved at its end (`packaging/iso/network.sh`): a file for each interface Debian's installer set up in `/etc/network/interfaces` (DHCP, or the static address it was given), the others none, so they stay down; both enabled, `/etc/resolv.conf` a link to systemd-resolved's stub, which knows the name servers on the first boot; ifupdown keeps the loopback only. **A Debian with ifupdown** is taken over on request (`POST /network/takeover`, `system.manage`, audited as `network.take_over`), in a job: systemd-resolved installed if missing (4.10); zendiloosd reads `/etc/network/interfaces` and what it sources from `interfaces.d` (the agent hands them over with a digest) and translates each interface ifupdown brings up (`auto`, `allow-hotplug`) that is a wired card here: `inet dhcp`, `inet static` (address, netmask, gateway, `dns-nameservers`; without them the name servers in force), `inet6 auto` and `inet6 dhcp` (router advertisements). It refuses anything else, naming file, line and why, and then leaves everything as it is: bonds, bridges, VLANs, wireless, commands run on the way, search domains, mappings, other sources, cards brought up only by hand. The agent refuses a takeover if the files changed since; it writes a record of everything it changes as it was (`/etc/network/zendiloos-takeover.json`, root's alone: `interfaces`, `resolv.conf` as file or link, the interfaces' systemd-networkd files, whether systemd-networkd and systemd-resolved were enabled and running), then stops `networking.service` (leases released, addresses removed), leaves ifupdown the loopback, links `resolv.conf` to the stub and enables and starts systemd-resolved and systemd-networkd. Not confirmed within two minutes, or should the agent or the machine restart first, ifupdown gets everything back from the record, from wherever the takeover got to: systemd-networkd first drops the static addresses it set (they would outlive it and stop ifupdown from setting them), it and systemd-resolved are stopped and disabled as they were, the files return, `networking.service` is restarted, and the record goes last, so a revert cut off is done again at the next start. Confirmed, the record goes; `interfaces.zendiloos-previous` and `resolv.conf.zendiloos-previous` stay for people. What systemd-networkd set in the kernel for IPv6 (router advertisements taken in user space) stays until the next boot after a revert | declarative files the agent writes atomically; verified in the second test VM (`just vm-network-test`): a setting that cuts the VM off goes back by itself; the card's file lost by hand is told after zendiloosd restarts, written back with confirmation, and kept when lost again; the takeover in a throwaway VM made an ifupdown machine (`just vm-network-takeover-test`): kept once confirmed, given back unconfirmed and after the agent restarted; bridges, bonds and VLANs in a throwaway VM with two spare cards (`just vm-network-bridge-test`): the NAS keeps its address over a bridge, one unconfirmed goes with its link, by the timer and as the agent starts, frames bridged to a stand-in for a VM pass Docker's FORWARD chain, a bond and a VLAN come up and go, the firewall's and Docker's tables as before; the ISO's switch by `just iso-network-test` and `just iso-test` |
| Machine's name | set by the installer (Debian's `netcfg`: `/etc/hostname`, and `127.0.1.1 <name>` in `/etc/hosts`, or the address it was given with the name and domain), changed on the control panel's Info page (`GET` and `PUT /system/hostname`, `system.manage` to change, audited as `system.host_name_update`). A name is one label as RFC 1123 allows (letters, digits and `-`, neither first nor last a `-`), not digits only and not `localhost`, kept in lowercase, and **at most 15 characters**: NetBIOS's limit, so the one name holds everywhere, as `<name>.local` (Avahi), as the SMB name Windows lists (Samba's NetBIOS name, which it takes from the host name in uppercase, and WS-Discovery, which asks Samba for it) and in the certificate; a name set elsewhere may be longer and is shown as it is. The agent (`SetHostName`, the protocol's `HostName`: a label of up to 63) has systemd-hostnamed set it (`hostnamectl hostname`: the static name and the kernel's; the agent and zendiloosd run in UTS namespaces of their own, `ProtectHostname`, so zendiloosd reads the name from `/etc/hostname`), puts the new name in `/etc/hosts` where it named the old one (a domain after it kept; Debian's `127.0.1.1` line made if no line names it; a link left alone), makes the machine's own, self-signed certificate anew for it (`<name>`, `<name>.local`, `localhost`, as `make-certificate` does; built beside the pair, swapped in, `nginx -t` and a reload if nginx serves it, the pair before back if nginx refuses; an issued certificate is never touched), and restarts what reads the name when it starts, only if it runs: `avahi-daemon`, `smbd` (open SMB connections end), `wsdd2`, in that order. `zendiloos.conf` names no host, so Samba's configuration is not written again; no unit file or link changes, so systemd is not reloaded. Within zendiloosd the notifications' subject follows (`HostNameObserver`). The interface says that the browser meets a new certificate and offers the new address when it was reached by the old name. A DHCP server learns the new name when the lease is renewed. The setup and sign-in pages show the name, so that several machines are told apart before anyone has signed in: `GET /api/v1/system/name` is public and tells it only to a client on the NAS's own network (the rule of the open setup, 4.1), where Avahi, Samba and the certificate announce it anyway; anyone else gets none. Tested in the agent against fake programs, by the API's tests and in the second test VM (`just vm-hostname-test`) | the NetBIOS name could be written into `zendiloos.conf` instead (and a longer host name cut there), but then Samba's name and the host name part; the self-signed certificate could be kept, but it would name a host that no longer answers |
| Network discovery | Windows finds the NAS through WS-Discovery (`wsdd2`), macOS through Bonjour (Samba announces `_smb._tcp` through `avahi-daemon`); both recommended by the package. NetBIOS stays off | without them, clients only reach the NAS by address; `wsdd` itself is not in Debian 13 |
| File services | SMB (Samba, with `wsdd2`) and NFS (`nfs-kernel-server`) are switched on and off in the control panel; off means stopped and disabled, so it stays off after a restart, while shares, exports and permissions stay. The workgroup and the oldest SMB version answered (2.0, 2.1 or 3; SMB1 never) are written as global settings at the top of `/etc/samba/zendiloos.conf`, which `smb.conf` includes at the end of `[global]`; both are applied with the shares in one agent request, and if the host refuses, the previous settings return | per-share protocols would multiply what can go wrong for little gain; FTP, rsync and WebDAV are not offered until there is a reason |
| SSH | the SSH server as a service in the control panel (Terminal): on or off (off means stopped and disabled; open sessions stay, as `KillMode=process`), its port (22, or a free one from 1024; zendiloosd refuses a port another program listens on, read from `/proc/net/tcp`), and whether root and passwords may log in, each either set or left as the system has it. The settings go to `/etc/ssh/sshd_config.d/00-zendiloos.conf`, read before the other drop-ins, so they win; `sshd -t` checks them first and the previous file returns if it fails. On request a transient systemd timer turns SSH off again after up to a day; SSH is then enabled only until the next boot (`enable --runtime`), as the timer itself is gone after a restart, so a restart turns it off too. Who may log in is Debian's: the accounts of the interface have no shell | keys, users allowed to log in and a web terminal belong to Debian's configuration or later |
| Time | time zone, network time on or off, up to four time servers (a drop-in of systemd-timesyncd; none: Debian's pool) or the clock set by hand, all through systemd-timedated (`timedatectl`); shown and set in the NAS's zone, not the browser's. Schedules follow a new zone within five minutes (`jiff` keeps the system zone that long), apps when they next start (they get `/etc/localtime`) | the agent may not set the clock itself (`ProtectClock`); timedated is what `timedatectl` and every desktop use |
| UPS | Network UPS Tools from Debian (`nut-server`, `nut-client`, `nut-snmp`), installed from the control panel's UPS page; zendiloOS writes their files and reads the state, NUT watches and shuts down (4.16) | apcupsd knows APC's UPSs only; a watcher of zendiloosd's own would not shut the NAS down while zendiloosd is stopped or being updated, and other NAS speak NUT, so one UPS serves several |
| Docker | Debian packages `docker.io` and `docker-compose`, installed as the App Center's Docker plugin (the package only suggests them) by apt in a unit of its own (4.10); apps are administrators' Compose projects (4.9) | security updates through Debian; Docker CE optional |
| Packaging | `debian/` with debhelper (compat 13 plus `dh-sequence-installsysusers`), built in a `debian:trixie` container with Rust from `rustup` and Node.js 24 pinned by checksum; for amd64 and arm64, the other architecture than the container's cross-compiled (`dpkg-buildpackage --host-arch`, Debian's cross compiler, the C library of that architecture through multiarch for linking and `dpkg-shlibdeps`) | native maintainer scripts, `sysusers.d`, `tmpfiles.d`, `dh_installsystemd`; works on any host with Docker; cross-compiling rather than building on an arm64 machine keeps one build environment, which a workstation runs in minutes and CI's x86 runners too, while the tests run natively on an arm64 runner (`docs/testing.md`) |
| Test VM | `scripts/vm.sh` (`just vm …`): Debian 13 cloud image verified by SHA512, cloud-init, 4 SATA disks with emulated SMART and 1 NVMe disk, ports on 127.0.0.1 unless `ZENDILOOS_VM_LISTEN` or `ZENDILOOS_VM_BRIDGE` (the LAN) say otherwise; several VMs side by side (`ZENDILOOS_VM`); spare network cards on a network of their own for bonds and VLANs (`ZENDILOOS_VM_EXTRA_NICS`); a VM installed afresh on its spare disks, pools and all (`reinstall`: a new system disk from the image, so a new machine id, host id and host key), for what a NAS's disks meet when it is installed anew | the cloud kernel suffices for ZFS (DKMS with its headers); it has no USB, so removable drives come over SCSI with the removable bit |
| Reproducibility | `rust-toolchain.toml`, `Cargo.lock`, `package-lock.json`, `SOURCE_DATE_EPOCH`, pinned build container; the installation ISO starts from Debian's netinst image, pinned by its SHA512 (`packaging/iso/netinst.env`); the appliance's images (phase 5) are to be built with mmdebstrap against a fixed snapshot.debian.org state, failing on any error | |
| Licensing | Zendiloos code under Apache-2.0; the installation ISO adds only zendiloOS's package and the answers to Debian's netinst image, and Debian's packages come from its mirror with their copyright files. Every release carries a software bill of materials beside its package, in its signed checksums (CycloneDX 1.5: the programs' crates, the npm packages in the web interface's bundle, the Debian packages the package names; `scripts/sbom.sh`, `docs/releases.md`); `docs/licences.md` is the overview made from it, and `just check` holds every licence to those `backend/deny.toml` allows (cargo-deny's own check, for the crates), or an exception for one package of the web interface (noVNC's MPL-2.0, the fonts' OFL-1.1). The build writes what it bundled beside the bundle (`licences.json`) with the licence texts (`third-party-licences.txt`). The appliance's image (phase 5) is to list its Debian packages with their versions too | required when distributing a Debian-based image |
| Quality | rustfmt, Clippy (`-D warnings` in CI), `unsafe_code = "forbid"` outside clearly bounded exceptions, `cargo deny`, ESLint, `vue-tsc --strict`, Vitest; coverage held by CI: backend 92 % of lines, every crate 90 % (the binaries' `main.rs` and zendiloosd's glue aside), contexts 93 %, domains 97 %; frontend 93 % of lines, its `.ts` logic 96 % (`scripts/coverage-check.sh`, `frontend/vite.config.ts`) | |
| Installation (phases 1–4) | an installation ISO per stable release: Debian 13's netinst image with an entry "Install zendiloOS", answers by preseeding (only language, keyboard, name, root's password and the system's disk are asked) and the release's package installed at the end, then the network handed to systemd-networkd (Network, above); ZFS built by DKMS on the machine (`docs/releases.md`) | Debian's own installer, nothing to maintain but the answers; the appliance's image (phase 5) can follow |
| Updates (phases 1–4) | Debian's packages through apt and zendiloOS's `.deb` from GitHub Releases, by channel (stable, test, development), checked daily, installed on request (4.10, `docs/releases.md`) | the appliance's image updates below build on the same releases and channels |
| Installation and updates | **[Phase 5]** recommendation: root image built with mmdebstrap as a signed image, A/B partitions, updates with systemd-sysupdate, automatic rollback via systemd-boot boot counting, data and `/var` on separate partitions (sketched in 4.10, "The system updated as A/B images") | alternatives: Debian installer with preseeding (apt based, not atomic), live-build with Calamares, mkosi. Decided in phase 5; the ground rule in 1.2 keeps all options open |

---

## 7. Open Questions

Decided on 2026-11-05: name `zendiloos`, unified accounts, target hardware
x86-64 NAS appliances and generic x86-64.
HTTPS by default: plain HTTP only redirects, the session cookie is
HTTPS-only, a self-signed certificate is created on the device;
certificates from Let's Encrypt come from the ACME plugin (4.11), and
installing a certificate of one's own follows later. CI: GitHub Actions
(`.github/workflows/ci.yml`) runs `just check` and `just deb` with the same
toolchain and tool versions as development, with actions pinned to commit
SHAs.

Decided on 2026-11-05 for storage (section 5): ZFS as the default, one
pool is one volume, mounted at `/volume<N>`, disks prepared with GPT and one
partition with a reserve.

Decided on 2026-11-06 for the installation ISO: it ships no ZFS module;
DKMS builds it on the machine, as on an existing Debian
(`docs/releases.md`).

Decided on 2026-11-12 for arm64 (issue #61): a package and an installation
ISO for arm64 beside amd64's in every release, the package cross-compiled
in the same build container, the NAS taking its own architecture's;
virtual machines stay amd64's until the domains are written for QEMU's
`virt` machine too.

1. ZFS in the appliance's image (phase 5): build it on the device, as the
   ISO does, or ship prebuilt modules, which needs a licensing review
   (CDDL and GPL).
2. Whether mdadm with Btrfs or ext4 comes back as an alternative to ZFS,
   e.g. for systems with little memory.
