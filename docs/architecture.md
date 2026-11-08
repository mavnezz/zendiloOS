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
zpool · zfs · sgdisk · wipefs · smartctl · nvme · useradd · smbpasswd · …
```

| Process | Runs as | Responsibility | Network |
|---|---|---|---|
| nginx | `www-data` | TLS, static files, rate limiting, proxying | yes (80/443) |
| `zendiloosd` | `zendiloos` | API, authentication, business logic, database, live data | Unix socket only |
| `zendiloos-agent` | `root` | privileged, validated single operations | none (`PrivateNetwork`) |

### 1.2 Two Deployment Modes, One Set of Packages

1. **Packages on an existing Debian 13** (phases 1–4): the `.deb` packages
   are installed on an existing system, at first in a test VM.
2. **Appliance from an ISO** (phase 5): the same packages, preinstalled in a
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
| `/etc/zendiloos/tls/` | self-signed certificate, created at installation or first boot (`zendiloos-certificate.service`) |
| `/etc/nginx/conf.d/zendiloos.conf` | HTTP → HTTPS redirect, TLS, security headers, rate limits, proxy to the API socket |
| `/run/zendiloos/api.sock` | `zendiloosd.socket`, `zendiloos:www-data 0660`: only nginx reaches the API |
| `/run/zendiloos/agent.sock` | `zendiloos-agent.socket`, `root:zendiloos 0660`: only zendiloosd reaches the system agent |
| `/run/zendiloos/files.sock` | `zendiloos-files.socket`, `root:zendiloos 0660`: the files agent, the same program with `--role files` |
| `/etc/samba/smb.conf` | installed from `/usr/share/zendiloos/smb.conf` only if it is Debian's unchanged default (kept as `smb.conf.debian`); it includes `/etc/samba/zendiloos.conf`, which the agent writes |
| `/var/lib/zendiloos/` | database, backups, setup code or the marker of an open setup (`setup-open`), console text (`0700`) |
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
| Bounded context | one core crate each: `identity`, `system`, `storage`, `sharing`, `apps`, `containers`, `updates`, `network` |
| Domain layer | entities, value objects, aggregates, domain rules and errors; pure Rust, no I/O |
| Application layer | use cases (commands and queries) and the **ports** they need, as traits |
| Primary (driving) adapter | `zendiloos-api`: HTTP and WebSocket |
| Secondary (driven) adapters | `zendiloos-infra`: SQLite repositories, agent gateways, host probes (`sysinfo`, `/proc`, journal) |
| Shared kernel | `zendiloos-kernel`: IDs, validated names, `Secret`, time, common port errors |
| Composition root | `zendiloosd`: reads the configuration and plugs adapters into ports |

| Crate | Role | May depend on (internal) |
|---|---|---|
| `zendiloos-kernel` | shared kernel: time, `Secret`, request context, the audit entry and its port, what runs in the background (`RunningWork`), common port errors | – |
| `zendiloos-identity` | accounts and groups with their Linux users and groups, roles and permissions, sessions, passwords, throttling, audit | kernel |
| `zendiloos-system` | system information, metrics, power, logs | kernel |
| `zendiloos-storage` | disks, SMART, ZFS pools and volumes, plans for erasing disks, monitoring; mdadm arrays read-only | kernel, jobs, notify |
| `zendiloos-sharing` | SMB and NFS shares, share permissions, and the file manager (share contents as the signed-in user) | kernel, jobs, notify |
| `zendiloos-apps` | app catalogs and their sources, their apps filled in as templates of administrators' projects, apps installed the earlier way (4.9) | kernel, jobs |
| `zendiloos-containers` | the Docker engine as administrators see it: containers, images, networks, Compose projects (administrators' own with their file in a share), use over time, newer images on a schedule, moving Docker's data (4.9) | kernel, jobs |
| `zendiloos-updates` | updates of the NAS: Debian's packages and zendiloOS's releases from GitHub by channel, checked daily, installed on request (4.10) | kernel, notify |
| `zendiloos-network` | interfaces, addresses, default routes, name servers (read-only until M6); later IP configuration, DNS, hostname | kernel |
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
      sharing · apps                       │
      network                              │
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
    ├── ports.rs       traits the context needs: PoolRepository, DiskInventory, …
    ├── commands/      use cases that change state, through the domain: CreatePool, …
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
- **Domain events** (for example `ShareCreated` → regenerate the Samba
  configuration) are published in-process through a port; no broker.

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
| Routing | Vue Router: `/setup`, `/login`, `/` (desktop) | apps are opened through the window store, not through URLs |
| i18n | vue-i18n; English first (the default and the source of all texts), German as a second language chosen in the settings | retrofitting internationalization is expensive |
| Icons | lucide-vue-next | matches shadcn-vue |
| Tests | Vitest with Vue Test Utils, later Playwright for end-to-end tests | |
| Formatting and linting | Prettier; ESLint with the Vue and TypeScript rules and `import-x/no-restricted-paths` for module boundaries | Prettier formats, ESLint checks code only |

```text
frontend/src/
├── main.ts
├── app/                  composition root: App.vue, router, guards, plugins
├── desktop/              shell
│   ├── windowManager.ts  pure logic (open, focus, minimize, move, resize,
│   │                     clamp geometry), unit tested with Vitest
│   ├── windows.store.ts  Pinia store around windowManager, layout in localStorage
│   ├── apps.ts           app registry: id, title, icon, window size, permission,
│   │                     lazily loaded component or sections
│   ├── Desktop.vue · Taskbar.vue · Launcher.vue · AppWindow.vue
│   └── NotificationCenter.vue
├── features/             one module per backend bounded context
│   ├── auth/  system/  storage/  shares/  files/  users/  containers/  network/
│   │   ├── api.ts        query/mutation composables (vue-query)
│   │   ├── model.ts      types and pure logic
│   │   └── components/   components and pages
└── shared/
    ├── api/              fetch client (CSRF, error format), WebSocket client, generated schema
    ├── ui/               shadcn-vue components, own building blocks (meter, sparkline, confirm dialog)
    ├── lib/              formatting, helpers
    └── i18n/             translations
```

- Features do not import from each other, only from `shared/`.
  `desktop/apps.ts` composes feature pages into apps; features know nothing
  about windows. Enforced with an ESLint rule (`import/no-restricted-paths`).
- The desktop's apps are the file explorer, the App Center, Docker and the
  control panel. Storage (overview, volumes, pools, disks), the resource
  monitor and the logs (system log, audit log) are pages of the control
  panel, in its groups *Storage* and *Monitoring*, each shown to the
  accounts with the permission it needs (`storage.view`, `system.view`,
  `logs.view`); features hand them over as pages, never with a frame of
  their own, so a window has one navigation. The dock's resource meters
  open the monitor's page.
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
- **One WebSocket per browser tab** (`/api/v1/ws`) with topic subscriptions
  instead of one connection per data stream:

  ```json
  → {"type":"subscribe","topic":"metrics"}
  ← {"type":"event","topic":"metrics","data":{…}}
  ← {"type":"event","topic":"jobs","data":{"id":"…","progress":0.42}}
  ← {"type":"event","topic":"notifications","data":{…}}
  ```

  A single sampler task produces metrics (every 2 s) and distributes them
  through `tokio::sync::broadcast`/`watch`; the load does not depend on the
  number of open browsers.
- In production `zendiloosd` listens on `/run/zendiloos/api.sock`, during
  development on `127.0.0.1:8080` (Vite proxies to it).

### 3.2 zendiloosd ↔ zendiloos-agent

| Property | Decision |
|---|---|
| Transport | Unix sockets created by systemd (`root:zendiloos 0660`), agents socket-activated: `/run/zendiloos/agent.sock` for the system agent (disks, ZFS, accounts, server configuration), `/run/zendiloos/files.sock` for the files agent (share contents); each refuses the other's requests |
| Authentication | file permissions + `SO_PEERCRED`: only the UID of `zendiloos` is served |
| Framing | one request per connection; JSON lines (NDJSON); size limits (requests 2 MiB, responses 16 MiB); before the answer of a long operation, any number of `progress` lines with the step it is at (`creating_pool`) and how far in percent, which become the job's step in zendiloosd and are named in the interface |
| Operations | closed `enum Request` with typed arguments validated during deserialization; `deny_unknown_fields` |
| Responses | a single response; **[Later]** a stream of frames (progress, logs) ending with a result |
| Files | for downloads and uploads the files agent passes the opened file's descriptor via `SCM_RIGHTS`, attached to the response line (see 4.4) |
| Version | `proto_version` handshake field; API and agent ship from the same package version, a mismatch is rejected |
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
  environment, no stdin except for secrets (passwords never on the command
  line), timeout, output limit (output over it ends the program at once;
  what it says on stderr past its limit is read and dropped, so it never
  waits on its pipe), each in a process group of its own, killed whole on a
  timeout. Currently: `systemctl`, `lsblk`, `smartctl`,
  `zpool`, `zfs`, `sgdisk`, `wipefs`, `udevadm`, `useradd`, `userdel`,
  `groupadd`, `groupdel`, `gpasswd`, `smbpasswd`, `testparm`, `exportfs`,
  `setfacl`.
- **Reads** of fixed files where no program is needed: `/proc/mdstat`.
  Although it is world-readable, the agent provides it, so that all storage
  state comes from one place and the simulation can provide all of it
  consistently.
- **Files** only at fixed locations known to the agent code, written
  atomically (temporary file, `fsync`, `rename`), for example
  `/etc/samba/zendiloos.conf`, `/etc/exports.d/zendiloos.exports`,
  `/etc/systemd/network/50-zendiloos-*.network`.
- **Mounts** through generated systemd mount units that the agent asks
  systemd to start. The mount then lives in the host's namespace (the
  hardened agent has its own), dependencies on Samba/NFS are explicit, and
  mounting at boot needs no `fstab` edits.
- **systemd** through `systemctl` with fixed arguments (`reboot`,
  `poweroff`, reloading `smbd` and `nfs-server`). What has to happen in the
  host's network namespace runs as a unit of the package: the kernel's NFS
  server keeps its export cache per namespace, so releasing a share's
  exports before unmounting it starts `zendiloos-nfs-flush.service`
  (`exportfs -r`, `exportfs -f`). **[Later]** D-Bus (zbus) once operations need more than
  queueing a job, such as following a unit's state.
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
| Signed-in user without admin rights | other users' files, privilege escalation | server-side permission checks, file access with the user's own kernel permissions |
| Web page in the same browser | CSRF, WebSocket hijacking | `SameSite=Strict`, CSRF header, origin check including the WebSocket upgrade, strict CSP |
| Compromised `zendiloosd` | root | the agent offers only validated domain operations, no generic execution; system disk protected. **Accepted limit:** an administrator may run any Compose project and give an app the Docker socket (4.9), and the agent carries that out as asked, so whoever controls `zendiloosd` controls Docker and with it the machine; an administrator's account is as good as root, as on other NAS systems |
| Malicious container | host | Docker access only through agent policy (see 4.9) |

### 4.2 Accounts

**Decided: unified accounts.** One account is valid for the web interface,
SMB, the file manager, and later NFS and apps. "Admin" is a role, not a
separate account.

- The database is authoritative (role, groups, Argon2id hash for the web
  login, preferences).
- For every account the agent creates a Linux account: UID from 2000, group
  `zendiloos-users`, shell `nologin`, no home directory, locked Linux
  password. Files on shares therefore belong to the right user.
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
  range. Names the system already uses are refused (`name_taken`).
- Groups are Linux groups as well; the agent sets their members.
- Order of changes: the host first (where failures are most likely), then
  the database. A failed creation removes the Linux user or group again.
- Administrators cannot change their own role or access, delete
  themselves or reset their own password here (they use their settings),
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
| `account.self` (own password, sessions, preferences) | ✓ | ✓ | ✓ |

The frontend hides apps the user lacks the permission for; that is a
convenience. Enforcement happens only in the backend (axum extractor
`RequirePermission<P>`).

Everybody sees their own jobs (file transfers, deletions); those with
`storage.view` see all jobs, with what tools reported, and the
notifications, which are about pools, disks and shares. The WebSocket
topics `jobs` and `notifications` apply the same rules.

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
  folders and system, each with the most serious level it is raised at
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
  shares; the creator may read and write, nobody else yet). Its
  permissions hold everywhere: in the file manager, for apps and projects,
  over SMB and NFS. **Shared** is a flag of its own: only a shared folder
  goes into Samba's configuration and NFS's exports; one not shared is used
  on the NAS alone (a project's folder people may look into, say), keeps
  its NFS clients for when it is shared again, and loses nothing when
  shared or not. Time Machine needs one that is shared. Folders deeper
  down are plain folders and never shared on their own. Folders on a
  volume are not renamed: the dataset, its mount unit, Samba's and NFS's
  sections, the snapshots' names and whatever mounts its path (apps,
  projects, uploads under way) would all have to follow at once.
- Share permissions (per account or group: no rule, deny, read, read and
  write) live in the database and are applied twice: in the Samba
  configuration (`valid users`, `invalid users`, `read only = yes`,
  `write list`) and as POSIX ACLs on the share's directory (owner root,
  mode 0770, an entry and a default entry per account or group). The same
  rules therefore hold for SMB, NFS and the file manager. A share nobody
  may use is not offered at all: Samba would read an empty `valid users` as
  "everybody". Administrators need a rule like everyone else.
- **Which rule wins:** a denial, the account's own or one of its groups';
  then the account's own rule, as it is more specific; then the broadest
  grant of its groups. POSIX ACLs say the same for accounts (a named user
  entry comes before the group entries), but cannot deny a group: its
  members get a named entry without permissions of their own instead, so a
  member's own grant is replaced. Samba keeps the group itself in
  `invalid users` and follows its members. When a denied group's members
  change, or the group is deleted, the Sharing context hears it from the
  Identity context (`GroupObserver`, bridged in infrastructure) and
  rewrites the entries of the shares that deny it, existing files in a job;
  where the group may write, the share's folder is set again, which makes
  the new members' recycle bins.
- **Personal folders:** one dataset `<pool>/zendiloos.homes` on one pool,
  mounted at `/volume<N>/@homes` (root, 0711); each account given one has
  `@homes/<name>` (its own, 0700) and a ZFS `userquota`. Samba offers them
  as one share `home` (`path = …/@homes/%U`, `valid users` the accounts
  given one), with a recycle bin like the shares; the name `home` is
  therefore no longer free for shares. In the file manager it is share 0,
  which the files agent resolves to the folder of the user it acts as and
  nobody else's. Turning them off, or taking one away, leaves the files.
  A folder of the name that belongs to another UID (an earlier account of
  the same name) is moved aside (`<name>.previous-<uid>`) and a fresh one
  made, so the new account is never locked out of its own folder.
- Changed rules apply to the share's directory at once and to existing
  files in a job, except the users' recycle bins. Access is taken away
  from the whole tree before any is given, so a revocation holds even if
  adding fails; the job may run for hours. The job applies the rules the
  database has when it gets to run, never those of the change that
  started it: zendiloosd runs one such job per share at a time, each reads
  the rules once it is its turn, and while one is under way a change of
  that share's rules is refused ("try again once that has ended"; changes
  of an app's access and of a denied group's members too), so the last job
  always gives the last rules, whatever order the jobs ran in.
- **Owners:** the kernel asks a file's owner entry (`user::`) before any
  named one, and lets the owner change mode and ACL; a named entry cannot
  take an owner's rights away. So in a share only those who may write own
  anything: when the rules are applied to the tree, what an account
  zendiloOS manages (UIDs 2000 to 59999, people and apps, accounts gone
  since among them) owns passes to root (its group stays) unless the
  account may write, by its own rule or, without one, by a group's. That
  takes in its recycle bin and private folders with what is in them, which
  stay private, now root's alone, and are not given back when the account
  may write again (an administrator can; a new bin is made). Files of other
  UIDs (system accounts, the users containers run as) keep their owners.
  Over SMB, Samba's `read only` and `write list` hold besides.
- The files agent walks the tree itself, depth first, by descriptor
  (`O_NOFOLLOW`, only the folders on the
  way down open, each read as it goes, never held whole: a folder of
  millions of entries takes no more memory than a small one), and changes
  each ACL through its descriptor
  (`system.posix_acl_*` via `/proc/self/fd`, the edits of `setfacl -x/-m`
  with the mask recomputed), so a folder swapped for a link meanwhile
  changes nothing. Links, other file systems, what is neither folder nor
  file are left alone, and so are the entries of the bins and of private
  folders (only their owner may enter: a bin a writer renamed, one passed
  to root) with what is in them, whose owners alone change; an entry that
  cannot be changed, or a folder nested deeper than 256 to look into,
  does not stop the others, the job fails at the end naming one.
  One change per share runs at a time; a change of a share's folder waits
  15 seconds at most for one going through its files, then is refused
  ("try again"), while other shares are not held up. Something not a
  folder in `.recycle`'s place is moved aside (`.recycle.not-a-folder-…`).
- **Recycle bin:** Samba's `vfs_recycle` moves files deleted over SMB to
  `.recycle/<user>/<folders>/<name>` in the share (`Copy #N of <name>` if
  taken), hidden but not vetoed, so users can restore from it. Each user's
  bin is private (0700). The agent creates `.recycle` itself with the
  share's ACL: created by Samba, it would get the private mode, every other
  user's bin could not be created, and Samba deletes files for good when it
  cannot move them. The file manager deletes into the same bins; inside a
  bin, deleting is for good, at any depth. It also restores from the
  user's own bin, any entry at any depth: back to where it was deleted
  from (`keeptree` keeps the folders), without the `Copy #N of ` of a
  second deletion, of the entry or of a folder on its way (what is inside a
  folder deleted twice goes back into that folder), folders that are gone
  made again, and a taken name gets a free one beside it; emptying the
  bin deletes `.recycle/<user>` for good.
- **File manager: the kernel checks, not our code.** `zendiloosd` cannot
  read shares itself. Every operation goes to the files agent, which runs
  it on a new thread that first takes on the signed-in user's credentials
  (`setgroups`, `setresgid`, `setresuid` as raw system calls, which on
  Linux change only the calling thread; the thread then has no
  capabilities left, which the agent checks) and ends with the operation.
  Only managed users qualify (UID 2000 to 59999, primary group
  `zendiloos-users`), never root or system accounts. On a removable drive
  the thread instead keeps root's access to files and nothing else
  (`CAP_DAC_OVERRIDE` and `CAP_DAC_READ_SEARCH` effective, every other
  capability dropped): a drive formatted elsewhere has owners nobody here
  has, and only an administrator's request names one; its root is opened
  only while a file system of its own is mounted there, never the tmpfs
  beneath. A copy or move between a drive and a folder on a volume runs on
  two threads, each with its side's rights: one walks the source as a copy
  does and sends what it finds in order, files already opened (at most 32
  pieces on their way), the other makes the copy from them, so what lands
  in a folder on a volume belongs to the person and takes the folder's
  inherited ACL exactly as an upload does, and only what the person may
  read there reaches a drive; a move removes the source once the copy is
  complete, and only what was copied (by device and inode). Paths are resolved
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
  is removed when it starts again, as that user. Likewise a move between
  shares removes its source only if it still is what was copied (device
  and inode), and a failed copy removes only the folder it made; folders
  are emptied in batches, never read whole. New files belong to the user and inherit the
  share's ACLs, exactly as over SMB. Copies, moves and deletions run as
  jobs. Verified end to end in the test VM (`just vm-sharing-test`).
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
  The files agent walks as the person, like the other walks: by
  descriptor, name by name, never following a link, never onto another
  file system, no deeper than 256 folders, each folder checked to be the
  one looked at before it is entered; what the person may not read is
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
  the files agent as the person may see them, walking as the search does.
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
  open without extra software. The files agent first plans it as the
  user: every entry and its size, so the archive's exact length is the
  `Content-Length`, and anything that cannot be read refuses the download
  instead of leaving files out silently. It then writes the archive as
  the user into a pipe whose reading end goes to `zendiloosd` like a
  downloaded file, following the plan to the byte (a file that grew is
  cut, one that shrank padded); should writing fail midway, the download
  ends short of its length and the browser reports it as failed. At the
  share's top the recycle bins are left out, everywhere uploads in
  progress; at most four folders are written at once.
- **The explorer** is one app for files and for the folders on the
  volumes (the Shares app of before is part of it): on the left a tree of
  places (the personal folder, the favourites, each volume with its folders
  down to any depth, the removable drives plugged in, and one recycle bin
  view that gathers the person's bins of every folder, each entry with the
  folder it was deleted from; folders in it open to any depth, and what is
  in them is restored or deleted for good from there too), on the right the
  details view (name, modified, type, size; sorted; chosen with clicks,
  Ctrl, Shift and Ctrl+A). What is typed into the address bar's search
  filters what is shown at once; in a folder, the NAS then looks through
  everything below it (see Search above), and what it found takes its
  place, each entry with the folder it is in and "Open its folder", which
  goes there with the entry chosen; Escape or "Clear the search" goes back. The toolbar and the context menu (a right click, or a
  long press on a touch screen, on an entry, on empty space or on a node of
  the tree) offer the same actions, from one list that says which belong
  where the explorer is and which the choice allows: open, download, new
  folder, upload, cut, copy, paste, rename, delete, restore and empty the
  recycle bin, copy back from a snapshot, favourites, and for a folder
  directly on a volume share on the network or stop, previous versions
  (snapshots looked into, and for those who manage shares taken, deleted
  and rolled back) and properties (general, permissions, sharing, NFS). Keys
  do the same (Del, F2, Ctrl+C/X/V/A, Enter, Backspace and Alt+arrows);
  entries dragged onto a folder move (Ctrl copies), files dragged from the
  computer upload. The volumes come from `GET /files/volumes`: someone who
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
  shared). `zendiloosd` lets only someone who manages storage name a drive,
  and only one the disks show mounted at its place now (the Storage
  context's `RemovableDrives::mounted`, through a port of the Sharing
  context); write-protected, it is read only. "Eject" (toolbar, and the
  drive's context menu in the tree) unmounts and powers it off, saying so
  when files on it are still open (`drive_in_use`); an ejected drive, or
  one that could not be mounted (its reason shown), offers "Mount". A
  drive ejected or pulled out while looked into sends the explorer back
  to where it starts.
- **Favourites** are folders an account keeps at hand, in its order (at
  most 50, in the database with the account and the share). Only those in
  shares the account may use are shown; the others are kept, should
  access come back.
- Alternative: the file manager runs with broad rights and checks ACLs
  itself. Simpler, but every gap in the checks becomes a data leak, and new
  files belong to the service account.
- NFS (v4 with `sec=sys`) trusts the client's UID. Exports are therefore
  restricted to the computers each rule names, and the interface states
  the risk. A rule names an IP address or network, a host name or a
  pattern with `*` and `?` as `exportfs` reads them (`*.example.com`;
  lowercase letters, digits, `-` and `.` only, checked by `zendiloosd` and
  again by the agent, so nothing reaches the exports file that it would
  read as a separator or an option), and says how: `rw` or `ro`; whose
  identity the requests keep, `root_squash` (root becomes the anonymous
  user; the default, and what every rule had before there was a choice),
  `all_squash` (everyone does) or `no_root_squash` (root stays root: the
  interface warns that the client's root is then root in the folder);
  `sync` or `async` (writes answered before they are on disk); `secure`
  or `insecure` (ports above 1024, which any user of the client may open,
  for clients such as macOS's Finder). The line the agent writes per
  client is `<host>(rw|ro,sync|async,no_subtree_check,<squash>,[insecure,]sec=sys)`.
  **[Later]** Kerberos.
- **Creating and deleting a share keep its record and its dataset in
  step.** A share that cannot be applied after it was stored goes again,
  record and dataset. Deleting hides the share first (it is offered no
  more and its name stays taken) and forgets it only once its dataset is
  gone; if applying fails, it is back as it was, and data that cannot go
  yet (busy, or zendiloosd stopped) goes when zendiloosd starts again.

### 4.5 Login, Sessions, CSRF

- Passwords: Argon2id (19 MiB, 2 passes). At least 10 characters, at most
  1024 bytes, must not contain the account name; no composition rules.
  **[Later]** check against a list of common passwords. Unknown account
  names cost as much time as wrong passwords, so response times do not
  reveal which accounts exist.
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
  `Sec-Fetch-Site` must be `same-origin` and `Origin` must match `Host` when
  the browser sends them; login and setup check this too, so a foreign page
  cannot sign a victim into an attacker's account. The WebSocket upgrade
  will get the same checks.
- Throttling in the backend, in memory: 20 failures per address within 15
  minutes, and the account lock as set under Security (5 failures within 15
  minutes by default, or off), for passwords and setup codes alike. An
  attempt counts as failed from its start until it ends otherwise, so
  attempts sent at once get no more checks than attempts one after
  another. Plus `limit_req` in nginx for login, setup and password changes.
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
- **[Later]** TOTP second factor, passkeys.

### 4.6 Agent Hardening

- A closed set of operations; no "run this command", no paths from the API
  that the agent does not check again against fixed roots.
- Operations that walk whole trees or disks (copying, moving and deleting
  trees, permissions of a whole share, preparing and destroying pools,
  fetching and running apps and projects, moving Docker's data) have two
  places of their own; quick ones have four. One user's large copies never
  keep others from listing, downloading or uploading. Each user has one
  long place at a time (a second copy waits behind the first, not in a
  place of everybody's), changes of whole shares' permissions have two of
  their own, and a folder download read at less than a mebibyte in five
  minutes is given up.
- **Nothing happens behind a caller's back:** a request still waiting for
  its turn, a place or a lock another operation holds, when its client
  gives up (zendiloosd's own timeout, its connection closed) is dropped,
  never carried out later. The client says how long it waits (`wait_ms`);
  the agent waits for the turn half of that at most, so what was asked has
  time to run and be answered, and refuses what has not started once it is
  asked to stop; what has started is seen through (`KillMode=mixed`: the
  programs it runs are not stopped with it, `TimeoutStopSec=2h`). zendiloosd waits two minutes for a quick system request
  and two hours for a long one, ten minutes and a day for files requests
  (copies, moves, permissions of a whole share), so once begun, a change is
  seen through and its outcome reported.
- Every change to a pool, scrubs included, requires its `zendiloos:volume`
  property: pools the owner made stay untouched.
- Known limits: with `erasable = "unused"`, a compromised `zendiloosd` could
  have any disk erased that nothing uses, including an exported pool or a
  cold backup; list the erasable disks on machines where that matters. On
  an existing Debian, any process running as `www-data` can reach the API
  socket and claim an address in `X-Real-IP`, passing by the per-address
  throttle.
- Recycle bins: when a share's permissions are applied, the files agent
  makes each writer's `.recycle/<user>` (directly or through a group):
  the user's own, mode 0700, without ACL entries. `.recycle` is sticky
  (1770 with the share's ACL): every writer makes their bin there, but
  only its owner may rename or remove it. Something of that name
  another writer made first is moved aside (`.<user>.not-theirs-…`), so
  it never receives the user's deletions; Samba itself does not check who
  owns a bin. The file manager also refuses a bin that is not the user's
  own and private, and empties a bin without removing it.
- Folder downloads are planned before the first byte (each entry once,
  its name and its folder's index, at most 100 000 entries and 32 MiB of
  names and link targets, both counted while each folder is read), one at a
  time per user and four in all, and given up when nobody reads for five
  minutes; the files agent may use 512 MiB (`MemoryMax`).
- Validated types are the only form in which values reach a command line
  (`DiskName` accepts only `sda`, `nvme0n1` and the like: no paths, no `-`
  prefix).
- `Secret` has a redacted `Debug`/`Display` and is never logged.
- Every operation is logged to the journal with request ID, peer UID/PID and
  result.
- systemd hardening: `PrivateNetwork`, `ProtectSystem=strict` with explicit
  `ReadWritePaths`, `ProtectHome`, `CapabilityBoundingSet` limited to what
  is needed, `SystemCallFilter`, `NoNewPrivileges` where possible. New
  operations extend this deliberately. As of M3: capabilities
  `CAP_SYS_RAWIO` and `CAP_SYS_ADMIN` (SMART through SG_IO and NVMe admin
  commands, partition tables, ZFS ioctls) and `CAP_CHOWN` (the shadow
  tools); devices closed except SATA/SAS/USB, NVMe and virtio disks and
  `/dev/zfs`; writable only `/etc` and Samba's state directories.
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
is built in and cannot be removed. Administrators add more, as with HACS:
a GitHub repository (`github.com/<owner>/<repo>`, read as
`https://<owner>.github.io/<repo>/v1/index.json`) or the https address of
an index in the same format. Adding one is confirmed after a warning, and
its apps carry "not from the zendiloOS catalog". `zendiloosd` fetches
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
the app declares. An app made into a project is then the administrator's
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
named: `privileged`, the Docker socket, folders outside the volumes, the
host's network, PID, IPC or user namespace, devices, extra capabilities,
no seccomp or AppArmor, settings not known as harmless, and what takes in
files whoever may write the share can change (`build`, built from the
share at every deployment; `env_file`, `extends`, `include`, read when it
is saved). Each must be confirmed, then and on every change;
the audit log records `risks_accepted`. The folders inside the project's
folder its binds mount (`./config`) are made, as far as they are missing,
through the file manager with the administrator's permissions when the
file is saved: Docker would make them as root, and a container run as
the administrator could not write to them. Deploying (pull, then
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
and no bind into a volume lets Docker make a missing folder (a pool not
mounted yet would get it on the system's disk). It keeps the result as
`/etc/zendiloos/projects/<project>.json` and runs Compose on that file with
an empty `.env`; `down` removes it again. A project made before this was
kept runs once its file is saved again. A project made elsewhere whose
Compose file lies in a share is not deployed here, for the same reason.
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
projects with a newer image are pulled and brought up again; containers
outside a project are only pulled for, apps of catalogs left to the App
Center, images pinned by digest never looked at. A container with a newer
image says so in the list, which also says how many can be updated; one
is updated by itself the same way, in a job (its project pulled and
brought up again, which makes its containers anew; outside a project only
its image fetched; an app installed the earlier way refused). The
interface asks first, saying how it goes for that container and that it
stops meanwhile and loses what is not in a mounted folder or volume,
unless the browser was told not to ask again.

**Moving Docker's data** to another pool zendiloOS made: Docker stops, the
dataset is copied with `zfs send -R | zfs receive -u` (up to six hours,
over a transport that waits as long), the mount unit, the drop-in and
`data-root` follow, Docker starts from the copy, and only then is the old
dataset destroyed; if the copy fails, or switching to the copy fails,
Docker goes back to its data as it was and starts again, never left
stopped. While it copies, other changes to pools go on (the pools' lock is
held only to check and to switch; one move at a time); the old pool cannot
be deleted meanwhile, Docker's data root still being there. Containers that
restart by themselves start again. The apps' datasets stay where they are.

**Docker's own data** (images, layers, containers) lives on a pool, not on
the system disk: turning apps on creates the dataset
`<pool>/zendiloos.docker` (the dot keeps it from ever being a share),
mounted by its own unit at `/volume<N>/@docker`; a drop-in
(`RequiresMountsFor`) keeps Docker from starting without it, so it never
writes into the empty folder beneath; and the agent sets `data-root` and
the `overlay2` driver in `/etc/docker/daemon.json`, keeping whatever else is
there. `overlay2` runs on OpenZFS 2.3 with native overlay diffs, without
the dataset per image layer of Docker's `zfs` driver. Docker comes from
Debian (`docker.io`, `docker-compose`, recommended by the package); its
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
   machine; `PUID`/`PGID` the administrator's UID and primary GID and `TZ`
   the time zone, both from the system agent (`HostFacts`, which also says
   which of the app's devices the machine has; the lines of the others are
   commented out, so the app still runs without a GPU or a Zigbee stick).
   Every value is filled in where Compose would interpolate it, in the
   values of the YAML read with their places in the text, quoted so that
   it reads back as exactly that value and with `$` doubled; comments and
   the rest of the file stay as written, and the result is read again and
   must hold the same document with the values in.
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

**Apps installed the earlier way** each got a dataset of their own
(`<pool>/zendiloos.apps/<id>` at `/volume<N>/@apps/<id>`), an account
`app-<id>` with access to the shares picked, kept by the Sharing context
(`share_app_access`), and a Compose file the agent checked against the
policy and keeps, normalized, readable by root only
(`/etc/zendiloos/apps/<id>.json`). They keep running: they are started
from that file (the agent walks each folder they mount again first),
stopped, their logs read, and removed; nothing installs, tries again or
updates an app so any more, and the agent has no steps left for it (no
preparing, checking, pulling or making folders). One whose installation
did not finish is *failed*, to be removed. Removing takes
the containers, images and kept file, destroys the dataset with the data,
then takes the account's share access and finally the account.

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
and the page follows the run from its record once zendiloosd is back. An
update does not start while background jobs run (`jobs_running`, the
detail names them, recorded as denied; asked again once the release has
come): a stopping agent first sees its running operations through, and
the upgrade would wait for that, for hours maybe. The
record is one file, which the next check replaces: so before a check or an
update starts, zendiloosd says how the last update went (once) and keeps
that run in its database, where the page still finds it. Tasks
are fixed: install Docker (`docker.io`, `docker-compose`, for the App
Store's own item), check (lists fetched, then `apt-get --simulate upgrade`
parsed into packages, security updates marked by Debian's security archive)
and update.

**Releases.** zendiloosd reads the repository's releases from GitHub's API
(`[updates]` in `zendiloosd.toml`): the package for this machine
(`zendiloos_<version>_<arch>.deb`), `SHA256SUMS` and its minisign signature
`SHA256SUMS.minisig`. A release's channel comes from GitHub: stable for a
release, test for a pre-release (`v1.2.0-rc.1`), development for the rolling
release `dev` built from every commit on main. Each channel takes the
steadier ones too; only a version newer than the running one, in dpkg's
order, is offered, so coming back from test to stable waits for a newer
stable release. Before an update zendiloosd downloads the three files to
`/var/lib/zendiloos/updates`. The runner trusts none of it: it copies them
out (never through a link, size-limited), checks the signature with the key
the package brought along (`/usr/share/zendiloos/release-signing.pub`, or
one an administrator put in `/etc/zendiloos`), the package's checksum, its
name, architecture and version, and installs only a version newer than the
installed one, so an old signed release cannot be brought back. A build
without a key takes no updates of zendiloOS. How releases are made and
signed: `docs/releases.md`.

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
changed package and an older release refused; Docker installed from the
store (`scripts/vm-docker-install-test.sh`).

---

## 5. Storage Concept

**Decided: ZFS is the default** (2026-11-05). One mechanism provides
redundancy, checksums with self-healing, snapshots, datasets and scrub, so
the interface and the agent deal with a single tool chain (`zpool`, `zfs`).

| Term | Implementation |
|---|---|
| Disk | detected with `lsblk --json`, identified by serial number and `/dev/disk/by-id`, never by `sdX` |
| Disk preparation | GPT with one partition of the type OpenZFS uses, from 1 MiB to 128 MiB before the end; the reserve lets a slightly smaller disk of the same nominal size replace a member |
| Pool | one zpool with one vdev: single disk, mirror (2 or more disks), RAIDZ1 (3+), RAIDZ2 (4+), RAIDZ3 (5+); `ashift=12`; members referenced by `/dev/disk/by-partuuid` |
| Volume | the pool's root dataset, mounted at **`/volume<N>`** (the lowest free number). One pool is one volume |
| Mounting | `mountpoint=legacy` and a systemd mount unit per volume (`volume1.mount`, after `zfs-import.target`): mounts happen in the host's namespace rather than the hardened agent's, and the boot order is explicit |
| Dataset defaults | `compression=lz4`, `atime=off`, `xattr=sa`, `acltype=posixacl` (for Samba ACLs), `dnodesize=auto`, `setuid=off`, `devices=off`; `normalization=formD`, `utf8only=on` (fixed at creation) **[Assumption]** |
| Share | a child dataset of the volume, mounted at `/volume<N>/<share>` by its own unit (`volume1-photos.mount`); created with `setuid=off` and `devices=off` also on pools made before those were defaults. Per share, stored in the database and set at every change and at startup: a quota on its own data (`refquota`, snapshots not counted, at least 1 GiB, never below what it holds), compression (`lz4`, `zstd` or off) and record size (16 KiB, 128 KiB, 1 MiB); what it holds comes live from ZFS |
| Snapshots | per share (child dataset), by hand or on schedules (hourly, daily, weekly, monthly, each keeping 1 to 100). Named after their UTC time, `zdl-YYYY-MM-DD_HH.MM.SS`, the form Samba's `vfs_shadow_copy2` reads, so Windows offers every snapshot as "Previous Versions" (`shadow:snapdir = .zfs/snapshot`); the schedules a snapshot was taken for and a note are ZFS user properties (`zendiloos:schedule`, `zendiloos:note`), the schedules themselves live in the database. `zendiloosd` looks every five minutes: a schedule that never took one, or missed its time while the machine was off, takes one at once; when several are due, one snapshot counts for all of them. Each schedule keeps its newest; a snapshot goes only when every schedule it was taken for lets it go, never one taken by hand or of a removed schedule. A failing schedule becomes one notification until it works again. The agent takes and destroys snapshots only of a share dataset on a pool zendiloos created, and the label's shape rules out ZFS's ranges and lists. The file manager looks into a snapshot as it was: the files agent opens `.zfs/snapshot/<label>` of the mounted share as a root of its own (the label's shape checked; opening it for reading makes ZFS mount it, which `RESOLVE_NO_XDEV` from the share would refuse), read only, and refuses every change in it; folders list, files download and preview, folders download as archives from it, and entries are copied back into the share, into the folder they were in and beside a name that is taken, so nothing is replaced (a job). A share can be rolled back to one of its snapshots (`zfs rollback -r`, the agent only on its own share datasets): what changed since is lost and so are the later snapshots, which the interface lists, so the share's name must be typed to confirm |
| Monitoring | `zpool status` (health, resilver and scrub progress, errors per device) every minute, SMART of disks holding data every six hours; changes become notifications (degraded or recovered pool, finished resilver or scrub, volume 80 or 90 % full, a disk's SMART getting worse); scrub monthly (second Sunday) by the cron job of `zfsutils-linux`, and on demand; a running scrub can be stopped (`zpool scrub -s`), a resilver cannot; while a scan runs, its speed and the time left are estimated as `zpool status` does (bytes issued in the current pass over its time minus pauses); the package depends on a cron daemon, which Debian's cloud image lacks. The storage overview answers "is my storage all right?" in one place: every concern (pool unavailable or degraded, unrepairable files, device errors, volume 80 or 90 % full, a disk's SMART verdict, no redundancy, no check for 40 days), the worst first; SMART verdicts are the monitor's last, so asking reads no disk; a disk can test itself (short or long SMART self-test, `smartctl --test`), its report shows the progress and the drive's own estimate of how long each test takes, and a running test can be stopped; a disk that sleeps, reports no SMART or cannot test itself is refused rather than woken. Self-tests also run on a schedule: one plan per kind (the long test weekly or monthly), the disks by serial number or all that can, starting from a local hour (the host's time zone, read with `jiff`) within four hours, so a NAS switched on late still tests the same night; the long test goes first, a disk already testing is left alone, a sleeping one is woken because the plan asked for it, and a failed test reaches the owner through the monitor's SMART notifications |
| Existing storage | mdadm arrays and Btrfs, ext4 or XFS filesystems found on the system are shown read-only (M2); zendiloos does not create or change them |
| Removable drives | USB sticks and disks, memory cards, for administrators only (`storage.manage`), never shared: a file system that passes the checks of 4.7 is named by its UUID and mounted at **`/run/zendiloos-removable/<id>`**, root's, outside the volumes (so no share, Samba or NFS export reaches it) and on a tmpfs (so nothing of it outlives a restart). `nosuid,nodev,noexec,noatime`; FAT (`utf8,shortname=mixed,flush`), exFAT and NTFS (the kernel's `ntfs3`, `windows_names`) have no owners of their own, so everything on them belongs to root and the group `zendiloos-users` (`uid=0,gid=…,fmask=0117,dmask=0007`): readable and writable by the accounts the files agent acts as, by nobody else, executable by nobody; ext4 keeps its owners and permissions, which the files agent overrides there with root's access to files, on threads of their own (4.4), so a drive formatted elsewhere is fully usable; what it writes there belongs to root. A drive that cannot be written (write-protected, NTFS that Windows left hibernated) is mounted read-only. Mounting happens in the host's namespace: the agent asks systemd for a transient mount unit (`systemd-mount --collect --fsck=no`), gone at the next boot and bound to its device, so systemd unmounts it when the drive is pulled out; `umount`, removing the place (`rmdir`) and powering off run in transient services (`systemd-run`), outside the agent's sandbox, which keeps `/run` read-only and only sees the mounts. `zendiloosd` mounts a drive when it sees it, as Windows does: whenever the drives are listed (the file explorer asks while it is open) and once a minute besides; the agent and `zendiloosd` speak `AF_UNIX` only, so no udev or netlink hook is needed. Ejecting unmounts (refused while files on it are open, `drive_in_use`), removes the place and, by default, takes the disk off the bus (this binary as `power-off-drive <disk>`: the SCSI device is deleted, which writes its cache out and stops it, then its USB device is removed, which powers the port off where the hub can), so it can be pulled out; without powering off, the drive stays plugged in and unmounted until it is pulled out or mounted by hand, e.g. to make a pool of it. A drive pulled out while mounted becomes a notification (`removable_pulled`), and what is left of it is let go of lazily (`umount --lazy` once its device is gone) and its place removed; so are empty places from before a restart. The API: `GET /storage/removable` (id, label, file system, device and disk, model, size, state `mounted`, `ejected`, `failed` with the agent's reason, or `unmounted`, `read_only`, `path`, space while mounted), `POST /storage/removable/{id}/mount` and `…/eject` (`power_off`, true by default). The simulation offers a USB stick (`1A2B-3C4D`, FAT) |
| Kernel module | `zfs-dkms` and `zfsutils-linux` from Debian contrib, built for the installed kernel with its headers |

| Option | Pros | Cons |
|---|---|---|
| **ZFS** (chosen) | self-healing with checksums and redundancy in one layer, snapshots, datasets, send/receive, mature | out-of-tree module (DKMS) that must be rebuilt for every kernel and may lag behind a new kernel; licensing questions for prebuilt modules in an ISO (see section 7) |
| mdadm + Btrfs | in-kernel, extremely well tested, all RAID levels | Btrfs detects bit rot on top of mdadm but cannot repair it; two layers to manage |
| Btrfs native RAID | self-healing, in-kernel | RAID 5/6 has known problems (write hole) |

---

## 6. Further Decisions

| Area | Decision | Alternatives / rationale |
|---|---|---|
| Base | Debian 13 "trixie" (kernel 6.12 LTS), amd64; arm64 from phase 5 | current stable; backports kernel for new hardware if needed |
| Target hardware | x86-64 NAS appliances with Intel CPUs and generic x86-64 systems with UEFI (self-built, mini PCs) | phase 5: boot medium, LEDs and fans (often via I²C), Intel GPU drivers |
| Development environment | Nix devShell (`flake.nix`, `.envrc`) for tooling on the development machine only; toolchain version from `rust-toolchain.toml` | the product contains no Nix; packages and images are built in a `debian:trixie` container |
| Database | SQLite (WAL) with sqlx; compile-time checked queries (`query!`), offline data in `backend/.sqlx` | after changing SQL or migrations run `just sqlx-prepare`; `just check` fails if the data is stale |
| Migrations | forward only, embedded, transactional; before each migration a copy of the database in `/var/lib/zendiloos/backup/` | safe migrations without down scripts |
| Configuration | static: `/etc/zendiloos/*.toml` (listen address, paths, log level, development flags); everything the admin manages lives in the database | |
| System metrics | `sysinfo` for CPU, memory, processes, sensors; an own parser for `/proc/diskstats` | sysinfo does not cover storage specifics |
| Network | systemd-networkd + systemd-resolved; on an existing Debian with ifupdown/NetworkManager display only. Per interface: IPv4 by DHCP or fixed (address, prefix, gateway), IPv6 from router advertisements or off, up to three name servers. The agent writes `/etc/systemd/network/05-zendiloos-<interface>.network` (before netplan's files), keeps the file it replaces as `….previous`, and has systemd-networkd reload and reconfigure the link over D-Bus (`busctl`: the agent's own network namespace has neither the host's links nor a working `networkctl`). A change is kept only if confirmed within two minutes, from the new address if it changed; otherwise the previous file returns, by a timer in the agent, and at the agent's start should it or the machine restart first. One change at a time | declarative files the agent writes atomically; verified in the second test VM (`just vm-network-test`): a setting that cuts the VM off goes back by itself |
| Network discovery | Windows finds the NAS through WS-Discovery (`wsdd2`), macOS through Bonjour (Samba announces `_smb._tcp` through `avahi-daemon`); both recommended by the package. NetBIOS stays off | without them, clients only reach the NAS by address; `wsdd` itself is not in Debian 13 |
| File services | SMB (Samba, with `wsdd2`) and NFS (`nfs-kernel-server`) are switched on and off in the control panel; off means stopped and disabled, so it stays off after a restart, while shares, exports and permissions stay. The workgroup and the oldest SMB version answered (2.0, 2.1 or 3; SMB1 never) are written as global settings at the top of `/etc/samba/zendiloos.conf`, which `smb.conf` includes at the end of `[global]`; both are applied with the shares in one agent request, and if the host refuses, the previous settings return | per-share protocols would multiply what can go wrong for little gain; FTP, rsync and WebDAV are not offered until there is a reason |
| SSH | the SSH server as a service in the control panel (Terminal): on or off (off means stopped and disabled; open sessions stay, as `KillMode=process`), its port (22, or a free one from 1024; zendiloosd refuses a port another program listens on, read from `/proc/net/tcp`), and whether root and passwords may log in, each either set or left as the system has it. The settings go to `/etc/ssh/sshd_config.d/00-zendiloos.conf`, read before the other drop-ins, so they win; `sshd -t` checks them first and the previous file returns if it fails. On request a transient systemd timer turns SSH off again after up to a day; SSH is then enabled only until the next boot (`enable --runtime`), as the timer itself is gone after a restart, so a restart turns it off too. Who may log in is Debian's: the accounts of the interface have no shell | keys, users allowed to log in and a web terminal belong to Debian's configuration or later |
| Time | time zone, network time on or off, up to four time servers (a drop-in of systemd-timesyncd; none: Debian's pool) or the clock set by hand, all through systemd-timedated (`timedatectl`); shown and set in the NAS's zone, not the browser's. Schedules follow a new zone within five minutes (`jiff` keeps the system zone that long), apps when they next start (they get `/etc/localtime`) | the agent may not set the clock itself (`ProtectClock`); timedated is what `timedatectl` and every desktop use |
| Docker | Debian packages `docker.io` and `docker-compose`, installed from the App Store's own item (the package only suggests them) by apt in a unit of its own (4.10), apps as Compose projects **[Phase 4]** | security updates through Debian; Docker CE optional |
| Packaging | `debian/` with debhelper (compat 13 plus `dh-sequence-installsysusers`), built in a `debian:trixie` container with Rust from `rustup` and Node.js 24 pinned by checksum | native maintainer scripts, `sysusers.d`, `tmpfiles.d`, `dh_installsystemd`; works on any host with Docker |
| Test VM | `scripts/vm.sh` (`just vm …`): Debian 13 cloud image verified by SHA512, cloud-init, 4 SATA disks with emulated SMART and 1 NVMe disk, ports on 127.0.0.1 only | the cloud kernel lacks some drivers; storage work (M3) may need `linux-image-amd64` in the VM **[Assumption]** |
| Reproducibility | `rust-toolchain.toml`, `Cargo.lock`, `package-lock.json`, `SOURCE_DATE_EPOCH`, pinned build container; system images built with mmdebstrap against a fixed snapshot.debian.org state, failing on any error | |
| Licensing | Zendiloos code under Apache-2.0; the ISO build produces a package manifest/SBOM and ships the copyright files | required when distributing a Debian-based image |
| Quality | rustfmt, Clippy (`-D warnings` in CI), `unsafe_code = "forbid"` outside clearly bounded exceptions, `cargo deny`, ESLint, `vue-tsc --strict`, Vitest; coverage held by CI: backend 92 % of lines, every crate 90 % (the binaries' `main.rs` and zendiloosd's glue aside), contexts 93 %, domains 97 %; frontend 93 % of lines, its `.ts` logic 96 % (`scripts/coverage-check.sh`, `frontend/vite.config.ts`) | |
| Installation (phases 1–4) | an installation ISO per stable release: Debian 13's netinst image with an entry "Install zendiloOS", answers by preseeding (only language, keyboard, name, root's password and the system's disk are asked) and the release's package installed at the end; ZFS built by DKMS on the machine (`docs/releases.md`) | Debian's own installer, nothing to maintain but the answers; the appliance's image (phase 5) can follow |
| Updates (phases 1–4) | Debian's packages through apt and zendiloOS's `.deb` from GitHub Releases, by channel (stable, test, development), checked daily, installed on request (4.10, `docs/releases.md`) | the appliance's image updates below build on the same releases and channels |
| Installation and updates | **[Phase 5]** recommendation: root image built with mmdebstrap as a signed image, A/B partitions, updates with systemd-sysupdate, automatic rollback via systemd-boot boot counting, data and `/var` on separate partitions | alternatives: Debian installer with preseeding (apt based, not atomic), live-build with Calamares, mkosi. Decided in phase 5; the ground rule in 1.2 keeps all options open |

---

## 7. Open Questions

Decided on 2026-11-05: name `zendiloos`, unified accounts, Nix devShell for
tooling only, target hardware x86-64 NAS appliances and generic x86-64.
HTTPS by default: plain HTTP only redirects, the session cookie is
HTTPS-only, a self-signed certificate is created on the device; installing
an own certificate and ACME follow later. CI: GitHub Actions
(`.github/workflows/ci.yml`) runs `just check` and `just deb` in the same Nix
shell as development, with actions pinned to commit SHAs.

Decided on 2026-11-05 for storage (section 5): ZFS as the default, one
pool is one volume, mounted at `/volume<N>`, disks prepared with GPT and one
partition with a reserve.

1. ISO and ZFS (M7): ship the DKMS sources and build on the device, as
   Debian does, or ship prebuilt modules, which needs a licensing review
   (CDDL and GPL).
2. Whether mdadm with Btrfs or ext4 comes back as an alternative to ZFS,
   e.g. for systems with little memory.
