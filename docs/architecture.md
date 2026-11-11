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
| `/etc/nfs.conf.d/zendiloos.conf` | fixed ports for NFSv3's helpers (mountd, statd, lockd), so the firewall can name them |
| `/etc/zendiloos/firewall/zendiloos.nft` | the firewall's rules, written by the agent once the firewall is on; `zendiloos-firewall.service` loads them with `nft` before the network comes up (4.12) |
| `/etc/zendiloos/apps/`, `/etc/zendiloos/projects/` | what the agent checked and runs, root's alone: apps installed the earlier way and administrators' Compose projects (4.9) |
| `/etc/zendiloos/vms/`, `/etc/tmpfiles.d/zendiloos-vms.conf`, `/run/zendiloos-vms/` | virtual machines (4.13): each domain's XML as the agent defined it; the consoles' folder, made at every boot, with QEMU's VNC sockets |
| `/etc/zendiloos/id-high-water` | the highest UID and GID the agent ever handed out (4.2) |
| `/var/lib/zendiloos-acme/` | acme.sh's accounts, certificates and files for HTTP challenges, owned by the system user `zendiloos-acme`; a DNS provider's credentials only while it runs, in `/run/zendiloos-acme/` (4.11) |
| `/run/zendiloos-removable/` | the places removable drives are mounted at, a folder each, named by the UUID of their file system |
| `zendiloos-nfs-flush.service` | releases a share's NFS exports in the host's network namespace (3.3) |
| `/run/zendiloos/api.sock` | `zendiloosd.socket`, `zendiloos:www-data 0660`: only nginx reaches the API |
| `/run/zendiloos/agent.sock` | `zendiloos-agent.socket`, `root:zendiloos 0660`: only zendiloosd reaches the system agent |
| `/run/zendiloos/files.sock` | `zendiloos-files.socket`, `root:zendiloos 0660`: the files agent, the same program with `--role files` |
| `/etc/samba/smb.conf` | installed from `/usr/share/zendiloos/smb.conf` only if it is Debian's unchanged default (kept as `smb.conf.debian`); it includes `/etc/samba/zendiloos.conf`, which the agent writes |
| `/var/lib/zendiloos/` | database, backups, downloaded releases (`updates/`), setup code or the marker of an open setup (`setup-open`) (`0700`) |
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
| Bounded context | one core crate each: `identity`, `system`, `storage`, `sharing`, `apps`, `containers`, `updates`, `network`, `certificates`, `firewall`, `vms` |
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
| `zendiloos-system` | system information, metrics, power, logs, the SSH server, time and time zone, the web interface's oldest TLS version | kernel |
| `zendiloos-storage` | disks, SMART and its self-tests, ZFS pools and volumes, removable drives, plans for erasing disks, monitoring; where each disk sits (the enclosure, a disk made to blink) and every disk seen; mdadm arrays read-only | kernel, jobs, notify |
| `zendiloos-sharing` | SMB and NFS shares, share permissions, and the file manager (share contents as the signed-in user) | kernel, jobs, notify |
| `zendiloos-apps` | app catalogs and their sources, their apps filled in as templates of administrators' projects, apps installed the earlier way (4.9) | kernel, jobs |
| `zendiloos-containers` | the Docker engine as administrators see it: containers, images, networks, volumes, Compose projects (administrators' own with their file in a share), use over time, newer images on a schedule, volumes backed up on a schedule and restored, moving Docker's data (4.9) | kernel, jobs, notify |
| `zendiloos-updates` | updates of the NAS: Debian's packages and zendiloOS's releases from GitHub by channel, checked daily, installed on request (4.10) | kernel, notify |
| `zendiloos-network` | interfaces, addresses, default routes, name servers; an interface's IP settings and name servers through systemd-networkd, changed with confirmation (6); the network taken over from ifupdown in a job, with confirmation (6); later the hostname | kernel, jobs |
| `zendiloos-certificates` | the ACME client plugin (acme.sh): accounts, challenges and certificates from Let's Encrypt, renewed daily; the web interface's ports and certificate, changed with confirmation (4.11) | kernel, jobs, notify |
| `zendiloos-firewall` | the firewall: one ordered list of rules (interface, service or ports, source, allow or deny) and what happens when none matches, enforced by nftables for the NAS and the ports Docker publishes, changed with confirmation; services nobody reaches, told (4.12) | kernel, notify |
| `zendiloos-vms` | virtual machines, a proof of concept: the plugin (libvirt and QEMU installed, their storage on a pool, the hardware check), VMs from an OS preset with their disk and an ISO copied from a share, made in a job, their state from libvirt and their consoles (4.13) | kernel, jobs |
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
  to the context that follows (Sharing); no broker.

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
  its own that only that app opens) and the control panel. Storage (pools, disks), the resource
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
  topic subscriptions (`metrics`, `jobs`, `notifications`) instead of one
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
  number of open browsers. A terminal in a container uses a WebSocket of
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
| Traceability | every operation carries a request ID that appears in the API's audit log and in the agent's journal |
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
  `zfs`, `sgdisk`, `wipefs`, `udevadm`, `umount`, `rmdir`, `rm`,
  `useradd`, `userdel`, `groupadd`, `groupdel`, `gpasswd`, `smbpasswd`,
  `testparm`, `exportfs`, `smbcontrol`, `sshd`, `docker`, `dockerd`,
  `setsid`, `openssl`, `nginx`, `nft`, `virsh` (and, in transient units,
  `systemd-tmpfiles` and this binary as `copy-vm-iso`, 4.13).
- **Reads** of fixed files where no program is needed: `/proc/mdstat`, and
  DMI's `sys_vendor` and `product_name` (`/sys/class/dmi/id`), which name
  the machine for the enclosure (5). Although they are world-readable, the
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
  `/etc/systemd/network/05-zendiloos-<interface>.network`; taking the
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
  `systemd-networkd` and `systemd-resolved`). What has
  to run outside the agent's sandbox runs as a unit: transient ones through
  `systemd-run` (apt, acme.sh, `nginx -t`, `nft -c`) and `systemd-mount`
  (removable drives), or a unit of the package. The kernel's NFS server
  keeps its export cache per network namespace, so releasing a share's
  exports before unmounting it starts `zendiloos-nfs-flush.service`
  (`exportfs -r`, `exportfs -f`). D-Bus is reached through `busctl`
  (systemd-networkd's `Reload` and `ReconfigureLink`, a link's
  `AdministrativeState`) and `timedatectl`.
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
- **Delivery.** Each notification raised is delivered on a task of its
  own, to the channels its kind is routed to, with 30 seconds per
  channel; the last outcome is kept and shown. A test goes to that
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
  `/sys` are only looked at, and a console's socket is connected to, which
  needs no write access to `/run`. What writes the volumes (an ISO copied
  or removed) runs in transient units that may write only `/volume<N>/@vms`.
  libvirtd and QEMU run outside, under libvirt's own confinement (AppArmor
  per VM, QEMU as `libvirt-qemu`).
- Folder downloads are planned before the first byte (each entry once,
  its name and its folder's index, at most 100 000 entries and 32 MiB of
  names and link targets, both counted while each folder is read), one at a
  time per user and four in all, and given up when nobody reads for five
  minutes; the files agent may use 512 MiB (`MemoryMax`).
- Validated types are the only form in which values reach a command line
  (`DiskName` accepts only `sda`, `nvme0n1` and the like: no paths, no `-`
  prefix).
- Taking the network over from ifupdown (6), the agent parses nothing: it
  hands ifupdown's files to zendiloosd as they are (only below
  `/etc/network`, never through a link, 256 KiB and 64 files at most) with
  their digest, and takes back interface settings of the protocol's types
  (`InterfaceName`, addresses, prefixes), each interface once, which it
  writes as systemd-networkd files of its own; it refuses them unless the
  files still have that digest. It changes only `/etc/network/interfaces`,
  `/etc/resolv.conf` and those files, after a record of how they were.
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
PNG and served as `image/png`.

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
(`zendiloos_<version>_<arch>.deb`), `SHA256SUMS` and its minisign signature
`SHA256SUMS.minisig`. A release's channel comes from GitHub: stable for a
release, test for a pre-release (`v1.2.0-rc.1`), development for the rolling
release `dev` built from every commit on main. Each channel takes the
steadier ones too; only a version newer than the running one, in dpkg's
order, is offered, so coming back from test to stable waits for a newer
stable release. A check picks its release for the channel set when it
ends, so a channel changed while apt fetched its lists counts, and a
release the channel does not take is neither shown nor installed. Before an update zendiloosd downloads the three files to
`/var/lib/zendiloos/updates`. The runner trusts none of it: it copies them
out (never through a link, size-limited), checks the signature with the key
the package brought along (`/usr/share/zendiloos/release-signing.pub`, or
one an administrator put in `/etc/zendiloos`), that the signature's trusted
comment is `zendiloos <version>` of this very package (one key signs every
build, the development build of each commit too), the package's checksum,
its name, architecture and version, and installs only a version newer than
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

**The kernel and ZFS.** ZFS is built by DKMS for each kernel. After an
update the runner looks for `zfs.ko` below the newest kernel's modules; if
it is missing the run fails with what to do, and the interface warns not to
restart, as the pools would not import. A newer kernel than the running one
is reported as waiting for a restart, which the administrator starts.

**Checks and notifications.** With the daily check on (the default), the
NAS looks once a day and says what it found as a notification, each finding
once; installing stays a click. A finished run is announced once as well.
Settings, the last check and what was said live in the database
(`system_update_settings`, `system_update_state`). Verified in the second
test VM (`scripts/vm-update-test.sh`): a check, zendiloOS updating itself to
a release signed with a test key and coming back with the new version, a
changed package and an older release refused; Docker installed as the
App Center's plugin (`scripts/vm-docker-install-test.sh`).

### 4.11 Certificates and the web interface

**The ACME client is a plugin** of the App Center, beside Docker: acme.sh
from Debian, installed by the package runner (task `install-acme`, as
Docker's) when an administrator asks. Its context, `zendiloos-certificates`,
keeps three things in the database (migration 0033, which also holds the
web interface's settings in `web_server`): **accounts** at an ACME
server (Let's Encrypt, or its staging server for trying out) with a contact
address (its domain kept in lowercase, and only characters acme.sh is
handed); **challenges**, how the server is shown a name is ours: DNS-01
through a DNS provider's API (a curated list of acme.sh's `dnsapi` scripts
with their variables: IONOS, Cloudflare, Hetzner, deSEC, INWX, netcup, OVH,
Gandi, Porkbun, Route 53, Hurricane Electric, Duck DNS, dynv6), or HTTP-01
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
and the certificate: the machine's own, self-signed one, or an issued one
that holds. Plain HTTP never serves the interface itself (the session cookie
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
still reach each other. A drop there is final; an accept never overrides
Docker's own drops. The agent has no network of its own (`PrivateNetwork`): `nft -c`
checks the file in a transient unit, and `zendiloos-firewall.service`
(`nft -f`, early at boot before `network-pre.target`, after Debian's
`nftables.service`, and again when that restarts or reloads, as its
`/etc/nftables.conf` flushes everything) loads it, reloaded after each
change. Off, the file only
removes the table; removing the package removes it too.

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

### 4.13 Virtual machines (proof of concept)

**What it is.** A third plugin of the App Center, *Virtual machines*:
libvirt and QEMU from Debian, installed by the package runner when an
administrator asks, the VMs' storage on a pool, and then a desktop app of
that name in which administrators (`vms.manage`) make, start, stop and
remove VMs and open their consoles. It is a proof of concept in scope, not
in care: every value is checked as in the other contexts, nothing a client
sends reaches a command line or a domain's XML as text, and zendiloosd
gains no privilege. Its context is `zendiloos-vms` (migration 0049).

**The hardware check.** The plugin's page says whether the CPU can run VMs
(`/dev/kvm` is there, with the flag `vmx` for Intel VT-x or `svm` for
AMD-V in `/proc/cpuinfo`) and whether an IOMMU is on (`/sys/class/iommu`
lists one: only needed to hand a device to a VM, which is not offered yet),
each with a line of explanation; the agent reads them (`VmHost`), as it
reads the machine's other facts, so the simulation can give them too.
Without KVM the installation is refused (`kvm_missing`): QEMU would run,
but at the speed of an emulator.

**libvirt, not QEMU run by hand.** libvirt starts QEMU as `libvirt-qemu`,
confined by an AppArmor profile it makes for each VM (`virt-aa-helper`:
only that VM's disk, ISO and console socket), hands the files it attaches
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
passt`. Not `libvirt-daemon-system`, as one would expect: in Debian 13
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
are left to the list above.

**The storage.** Setting up puts the dataset `<pool>/zendiloos.vms` on a
pool zendiloOS made (`setuid`, `devices` and `exec` off; the dot keeps it
from ever being a share), mounted by its own unit at `/volume<N>/@vms`
before libvirtd, which a drop-in (`RequiresMountsFor`) keeps from starting
without it, as for Docker's data. A VM's disk is a sparse zvol
`<pool>/zendiloos.vms/<id>-disk0` (`volmode=dev`: its partitions never show
on the host), virtio for the Linux preset and SATA for Windows and other
systems, which bring no virtio drivers. A pool with the VMs' storage on it
is not deleted (the agent refuses); moving the storage is not offered yet.

**A VM.** Its settings: a name, an OS preset (Linux, Windows, other) that
fills in vCPUs, memory, disk size and firmware, vCPUs up to the machine's
threads and memory up to its total (the administrator weighs what the NAS
keeps), UEFI (OVMF, Secure Boot off) or BIOS (SeaBIOS), one system disk,
an optional ISO, the network and autostart. The agent builds the domain's
XML from the checked protocol types alone (`VmSpec`) and keeps it as
`/etc/zendiloos/vms/zendiloos-<id>.xml` (`virsh define --validate`): q35,
the host's CPU, ACPI; virtio disk, network card, video, balloon and random
source for Linux; SATA, an e1000e card and VGA for the others; Hyper-V's
enlightenments and local time for Windows; a VNC console on a Unix socket
and nothing listening on a network. Making a VM is a job (`vms.create`):
the ISO copied, the disk made, the domain defined; what a failure leaves
goes again, its record too. Start, shut down (ACPI), restart (ACPI), force
off, autostart on and off, and delete with the name typed again (the domain
undefined with its NVRAM, the zvol destroyed, the ISO's copy removed) are
audited (`vms.install`, `vms.storage`, `vms.create`, `vms.start`,
`vms.shutdown`, `vms.restart`, `vms.force_off`, `vms.autostart_on`,
`vms.autostart_off`, `vms.delete`, `vms.console`).

**The network.** By default user-mode networking through passt: the VM's
connections leave from the NAS's own address as the NAS's sockets, through
its firewall's rules for what goes out; nothing reaches the VM (no ports
are forwarded). Or a Linux bridge of the machine's own (not Docker's
`docker0` or `br-<id>`, not libvirt's `virbr…`), on which libvirt puts the
VM's tap: the VM is on the LAN as a machine of its own. **[Open]** Docker
loads `br_netfilter`, so frames bridged to a VM pass the firewall's forward
chain; with a denying fallback (4.12) a VM on a bridge would need rules of
its own, which the firewall does not offer yet.

**ISOs never come from a share.** QEMU must not reach the shares, and
libvirt, which gives each file it attaches to QEMU's account while the VM
runs, must never change a file in one. So the ISO an administrator picks
in a share is copied into `/volume<N>/@vms/isos/<id>.iso` and only the copy
is attached, read-only, as a SATA CD drive booted first; it goes with the
VM. zendiloosd first opens the file through the file manager as that
administrator (`SharesForVms`), so a share or a file they may not read is
refused (`iso_denied`). The copy is the job's first step, with its
progress: the system agent cannot write the volumes, so it starts the
unit `zendiloos-vm-iso-<id>.service` (this binary as `copy-vm-iso`), which
may write only `/volume<N>/@vms` and has no capability but
`CAP_DAC_READ_SEARCH`. It opens the file beneath the share's root
(`openat2`, no links, no other file system, only a regular file, without
waiting on a pipe) and copies as `cp --reflink=auto` would: `FICLONE`
first, then `copy_file_range`, which OpenZFS 2.3 answers by cloning blocks
within the pool where its `zfs_bclone_enabled` lets it (cheap and
instant), else a plain copy; under a hidden name, renamed once complete and
on disk. Whoever may write the share could swap the file between the check
and the copy for another plain file in that share, which would be copied
in its place; administrators are as good as root anyway (S2).

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

**[Later]** Changing a VM once made (vCPUs, memory, the disk's size, a
second disk), changing or ejecting its ISO, snapshots and backups of its
zvol, moving the storage to another pool, handing devices to a VM (the
IOMMU), a TPM and Secure Boot with Microsoft's keys for Windows 11 and the
virtio drivers for Windows, rules of the firewall for VMs on a bridge, what
libvirt-guests does at a shutdown (Debian's default: guests shut down),
and pools that hold VMs named in `pool_in_use`.

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
| Dataset defaults | `compression=lz4`, `atime=off`, `xattr=sa`, `acltype=posixacl` (for the shares' one ACL), `dnodesize=auto`, `setuid=off`, `devices=off`; `normalization=formD`, `utf8only=on` (fixed at creation) |
| Share | a child dataset of the volume, mounted at `/volume<N>/<share>` by its own unit (`volume1-photos.mount`); created with `setuid=off` and `devices=off` also on pools made before those were defaults. Per share, stored in the database and set at every change and at startup: a quota on its own data (`refquota`, snapshots not counted, at least 1 GiB, never below what it holds), compression (`lz4`, `zstd` or off) and record size (16 KiB, 128 KiB, 1 MiB); what it holds comes live from ZFS |
| Snapshots | per share (child dataset), by hand or on schedules (hourly, daily, weekly, monthly, each keeping 1 to 100). Named after their UTC time, `zdl-YYYY-MM-DD_HH.MM.SS`, the form Samba's `vfs_shadow_copy2` reads, so Windows offers every snapshot as "Previous Versions" (`shadow:snapdir = .zfs/snapshot`); the schedules a snapshot was taken for and a note are ZFS user properties (`zendiloos:schedule`, `zendiloos:note`), the schedules themselves live in the database. `zendiloosd` looks every five minutes: a schedule that never took one, or missed its time while the machine was off, takes one at once; when several are due, one snapshot counts for all of them. Each schedule keeps its newest; a snapshot goes only when every schedule it was taken for lets it go, never one taken by hand or of a removed schedule. A failing schedule becomes one notification until it works again. The agent takes and destroys snapshots only of a share dataset on a pool zendiloos created, and the label's shape rules out ZFS's ranges and lists. The file manager looks into a snapshot as it was: the files agent opens `.zfs/snapshot/<label>` of the mounted share as a root of its own (the label's shape checked; opening it for reading makes ZFS mount it, which `RESOLVE_NO_XDEV` from the share would refuse), read only, and refuses every change in it; folders list, files download and preview, folders download as archives from it, and entries are copied back into the share, into the folder they were in and beside a name that is taken, so nothing is replaced (a job). A share can be rolled back to one of its snapshots (`zfs rollback -r`, the agent only on its own share datasets): what changed since is lost and so are the later snapshots, which the interface lists, so the share's name must be typed to confirm; the files get the permissions recorded now again, not those of the snapshot (4.4) |
| Monitoring | `zpool status` (health, resilver and scrub progress, errors per device) every minute, SMART of disks holding data every six hours; changes become notifications (degraded or recovered pool, finished resilver or scrub, volume 80 or 90 % full, a disk's SMART getting worse); scrub monthly (second Sunday) by the cron job of `zfsutils-linux`, and on demand; a running scrub can be stopped (`zpool scrub -s`), a resilver cannot; while a scan runs, its speed and the time left are estimated as `zpool status` does (bytes issued in the current pass over its time minus pauses); the package depends on a cron daemon, which Debian's cloud image lacks. The concerns, when there are any, are listed above the Pools and Disks pages under one verdict, the worst first: pool unavailable or degraded, unrepairable files, device errors, volume 80 or 90 % full, a disk's SMART verdict, no redundancy, no check for 40 days; a disk is named there as the pages name it, by its kernel name with its slot (where the enclosure places it) and its model or serial number, also a device of a pool, whose `/dev/disk/by-id` or `by-path` link (`wwn-…`, `ata-…_<serial>`, with or without `-part<N>`) or kernel name ZFS reports is matched to the disk it is on, by the link lsblk reports, the WWN, a serial number only one disk reports or the port; one no disk matches keeps the name ZFS gives it, which the concern keeps besides (`device`), and a disk's SMART notification names its slot too; SMART verdicts are the monitor's last, so asking reads no disk; a disk can test itself (short or long SMART self-test, `smartctl --test`), its report shows the progress and the drive's own estimate of how long each test takes, and a running test can be stopped; a disk that sleeps, reports no SMART or cannot test itself is refused rather than woken. Self-tests also run on a schedule: one plan per kind (the long test weekly or monthly), the disks by serial number or all that can, starting from a local hour (the host's time zone, read with `jiff`) within four hours, so a NAS switched on late still tests the same night; the long test goes first, a disk already testing is left alone, a sleeping one is woken because the plan asked for it, and a failed test reaches the owner through the monitor's SMART notifications |
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
| Base | Debian 13 "trixie" (kernel 6.12 LTS), amd64; arm64 from phase 5 | current stable; backports kernel for new hardware if needed |
| Target hardware | x86-64 NAS appliances with Intel CPUs and generic x86-64 systems with UEFI (self-built, mini PCs) | phase 5: boot medium, LEDs and fans (often via I²C), Intel GPU drivers |
| Development environment | tools installed on the development machine (README, "Development"): Rust from `rust-toolchain.toml` through rustup, Node from `.nvmrc`; CI installs the same versions | packages and images are built in a `debian:trixie` container |
| Database | SQLite (WAL) with sqlx; compile-time checked queries (`query!`), offline data in `backend/.sqlx` | after changing SQL or migrations run `just sqlx-prepare`; `just check` fails if the data is stale |
| Migrations | forward only, embedded, transactional; before each migration a copy of the database in `/var/lib/zendiloos/backup/` | safe migrations without down scripts |
| Configuration | static: `/etc/zendiloos/*.toml` (listen address, paths, log level, development flags); everything the admin manages lives in the database | |
| System metrics | `sysinfo` for CPU, memory, processes, sensors; an own parser for `/proc/diskstats` | sysinfo does not cover storage specifics |
| Network | systemd-networkd + systemd-resolved; NetworkManager is only shown. Per interface: IPv4 by DHCP, fixed (address, prefix, gateway) or off, IPv6 from router advertisements or off (both off: the interface switched off, `ActivationPolicy=down`), up to three name servers; an interface without a file of zendiloOS's (down, or never set up) is switched on the same way. The agent writes `/etc/systemd/network/05-zendiloos-<interface>.network` (before netplan's files), keeps the file it replaces as `….previous`, and has systemd-networkd reload and reconfigure the link over D-Bus (`busctl`: the agent's own network namespace has neither the host's links nor a working `networkctl`). DHCP names the card by its MAC address (`ClientIdentifier=mac`): systemd's default, a DUID from the machine ID, changes with every installation, and so do those of ifupdown's DHCP clients (dhcpcd's `duid`, dhclient's `-i`); and it asks for the address the card has (`RequestAddress=`), so a server that keyed the lease by another client's identifier hands it out again once that one released it. A change is kept only if confirmed within two minutes, from the new address if it changed; otherwise the previous file returns, by a timer in the agent, and at the agent's start should it or the machine restart first. One change at a time. **The installation ISO** hands the network to systemd-networkd and systemd-resolved at its end (`packaging/iso/network.sh`): a file for each interface Debian's installer set up in `/etc/network/interfaces` (DHCP, or the static address it was given), the others none, so they stay down; both enabled, `/etc/resolv.conf` a link to systemd-resolved's stub, which knows the name servers on the first boot; ifupdown keeps the loopback only. **A Debian with ifupdown** is taken over on request (`POST /network/takeover`, `system.manage`, audited as `network.take_over`), in a job: systemd-resolved installed if missing (4.10); zendiloosd reads `/etc/network/interfaces` and what it sources from `interfaces.d` (the agent hands them over with a digest) and translates each interface ifupdown brings up (`auto`, `allow-hotplug`) that is a wired card here: `inet dhcp`, `inet static` (address, netmask, gateway, `dns-nameservers`; without them the name servers in force), `inet6 auto` and `inet6 dhcp` (router advertisements). It refuses anything else, naming file, line and why, and then leaves everything as it is: bonds, bridges, VLANs, wireless, commands run on the way, search domains, mappings, other sources, cards brought up only by hand. The agent refuses a takeover if the files changed since; it writes a record of everything it changes as it was (`/etc/network/zendiloos-takeover.json`, root's alone: `interfaces`, `resolv.conf` as file or link, the interfaces' systemd-networkd files, whether systemd-networkd and systemd-resolved were enabled and running), then stops `networking.service` (leases released, addresses removed), leaves ifupdown the loopback, links `resolv.conf` to the stub and enables and starts systemd-resolved and systemd-networkd. Not confirmed within two minutes, or should the agent or the machine restart first, ifupdown gets everything back from the record, from wherever the takeover got to: systemd-networkd first drops the static addresses it set (they would outlive it and stop ifupdown from setting them), it and systemd-resolved are stopped and disabled as they were, the files return, `networking.service` is restarted, and the record goes last, so a revert cut off is done again at the next start. Confirmed, the record goes; `interfaces.zendiloos-previous` and `resolv.conf.zendiloos-previous` stay for people. What systemd-networkd set in the kernel for IPv6 (router advertisements taken in user space) stays until the next boot after a revert | declarative files the agent writes atomically; verified in the second test VM (`just vm-network-test`): a setting that cuts the VM off goes back by itself; the takeover in a throwaway VM made an ifupdown machine (`just vm-network-takeover-test`): kept once confirmed, given back unconfirmed and after the agent restarted; the ISO's switch by `just iso-network-test` and `just iso-test` |
| Network discovery | Windows finds the NAS through WS-Discovery (`wsdd2`), macOS through Bonjour (Samba announces `_smb._tcp` through `avahi-daemon`); both recommended by the package. NetBIOS stays off | without them, clients only reach the NAS by address; `wsdd` itself is not in Debian 13 |
| File services | SMB (Samba, with `wsdd2`) and NFS (`nfs-kernel-server`) are switched on and off in the control panel; off means stopped and disabled, so it stays off after a restart, while shares, exports and permissions stay. The workgroup and the oldest SMB version answered (2.0, 2.1 or 3; SMB1 never) are written as global settings at the top of `/etc/samba/zendiloos.conf`, which `smb.conf` includes at the end of `[global]`; both are applied with the shares in one agent request, and if the host refuses, the previous settings return | per-share protocols would multiply what can go wrong for little gain; FTP, rsync and WebDAV are not offered until there is a reason |
| SSH | the SSH server as a service in the control panel (Terminal): on or off (off means stopped and disabled; open sessions stay, as `KillMode=process`), its port (22, or a free one from 1024; zendiloosd refuses a port another program listens on, read from `/proc/net/tcp`), and whether root and passwords may log in, each either set or left as the system has it. The settings go to `/etc/ssh/sshd_config.d/00-zendiloos.conf`, read before the other drop-ins, so they win; `sshd -t` checks them first and the previous file returns if it fails. On request a transient systemd timer turns SSH off again after up to a day; SSH is then enabled only until the next boot (`enable --runtime`), as the timer itself is gone after a restart, so a restart turns it off too. Who may log in is Debian's: the accounts of the interface have no shell | keys, users allowed to log in and a web terminal belong to Debian's configuration or later |
| Time | time zone, network time on or off, up to four time servers (a drop-in of systemd-timesyncd; none: Debian's pool) or the clock set by hand, all through systemd-timedated (`timedatectl`); shown and set in the NAS's zone, not the browser's. Schedules follow a new zone within five minutes (`jiff` keeps the system zone that long), apps when they next start (they get `/etc/localtime`) | the agent may not set the clock itself (`ProtectClock`); timedated is what `timedatectl` and every desktop use |
| Docker | Debian packages `docker.io` and `docker-compose`, installed as the App Center's Docker plugin (the package only suggests them) by apt in a unit of its own (4.10); apps are administrators' Compose projects (4.9) | security updates through Debian; Docker CE optional |
| Packaging | `debian/` with debhelper (compat 13 plus `dh-sequence-installsysusers`), built in a `debian:trixie` container with Rust from `rustup` and Node.js 24 pinned by checksum | native maintainer scripts, `sysusers.d`, `tmpfiles.d`, `dh_installsystemd`; works on any host with Docker |
| Test VM | `scripts/vm.sh` (`just vm …`): Debian 13 cloud image verified by SHA512, cloud-init, 4 SATA disks with emulated SMART and 1 NVMe disk, ports on 127.0.0.1 unless `ZENDILOOS_VM_LISTEN` or `ZENDILOOS_VM_BRIDGE` (the LAN) say otherwise; several VMs side by side (`ZENDILOOS_VM`) | the cloud kernel suffices for ZFS (DKMS with its headers); it has no USB, so removable drives come over SCSI with the removable bit |
| Reproducibility | `rust-toolchain.toml`, `Cargo.lock`, `package-lock.json`, `SOURCE_DATE_EPOCH`, pinned build container; the installation ISO starts from Debian's netinst image, pinned by its SHA512 (`packaging/iso/netinst.env`); the appliance's images (phase 5) are to be built with mmdebstrap against a fixed snapshot.debian.org state, failing on any error | |
| Licensing | Zendiloos code under Apache-2.0; the installation ISO adds only zendiloOS's package and the answers to Debian's netinst image, and Debian's packages come from its mirror with their copyright files. A package manifest/SBOM comes with the appliance's image (phase 5) | required when distributing a Debian-based image |
| Quality | rustfmt, Clippy (`-D warnings` in CI), `unsafe_code = "forbid"` outside clearly bounded exceptions, `cargo deny`, ESLint, `vue-tsc --strict`, Vitest; coverage held by CI: backend 92 % of lines, every crate 90 % (the binaries' `main.rs` and zendiloosd's glue aside), contexts 93 %, domains 97 %; frontend 93 % of lines, its `.ts` logic 96 % (`scripts/coverage-check.sh`, `frontend/vite.config.ts`) | |
| Installation (phases 1–4) | an installation ISO per stable release: Debian 13's netinst image with an entry "Install zendiloOS", answers by preseeding (only language, keyboard, name, root's password and the system's disk are asked) and the release's package installed at the end, then the network handed to systemd-networkd (Network, above); ZFS built by DKMS on the machine (`docs/releases.md`) | Debian's own installer, nothing to maintain but the answers; the appliance's image (phase 5) can follow |
| Updates (phases 1–4) | Debian's packages through apt and zendiloOS's `.deb` from GitHub Releases, by channel (stable, test, development), checked daily, installed on request (4.10, `docs/releases.md`) | the appliance's image updates below build on the same releases and channels |
| Installation and updates | **[Phase 5]** recommendation: root image built with mmdebstrap as a signed image, A/B partitions, updates with systemd-sysupdate, automatic rollback via systemd-boot boot counting, data and `/var` on separate partitions | alternatives: Debian installer with preseeding (apt based, not atomic), live-build with Calamares, mkosi. Decided in phase 5; the ground rule in 1.2 keeps all options open |

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

1. ZFS in the appliance's image (phase 5): build it on the device, as the
   ISO does, or ship prebuilt modules, which needs a licensing review
   (CDDL and GPL).
2. Whether mdadm with Btrfs or ext4 comes back as an alternative to ZFS,
   e.g. for systems with little memory.
