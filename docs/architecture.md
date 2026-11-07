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
| `/var/lib/zendiloos/` | database, backups, setup code, console text (`0700`) |
| `/run/issue.d/zendiloos.issue` | the console text, written by the agent from its own template (0600): the login prompt shows the address and, until setup is done, the setup code. zendiloosd only names the code; nothing it controls is read by agetty |

`zendiloosd` uses `Type=notify` and is ordered before `getty.target`, so the
console prompt appears only once the setup code is written. All services
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
| `zendiloos-apps` | app catalogs and their sources, installed apps (Docker Compose projects under the policy of 4.9) | kernel, jobs |
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
| `zendiloos-notify` | notifications for administrators: a code with parameters, stored (the 500 most recent) and published; how far each account has read and cleared them; channels that carry them by email and to webhooks | kernel |
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
| Device on the LAN without an account | log in, take over during initial setup | throttling, setup code, no default password, TLS |
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
| `system.view` (dashboard, information) | ✓ | ✓ | ✓ |
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
set up, so that a failing disk is noticed without the interface open:
email over SMTP (TLS, STARTTLS, or unencrypted to a relay in the own
network, never with a password) and webhooks (one HTTP POST, as JSON with
the fields chat services, Gotify and home automation read, or as text
with a `Title` header for ntfy). Each channel carries everything, warnings
and errors (with the good news that ends them), or errors only, worded on
the server in English or German as the interface words them, the NAS's
name in the subject. Each notification is delivered on a task of its
own, with 30 seconds per channel; the last outcome is kept and shown, and
a test message answers with the server's reason when it fails. TLS is
rustls with ring and the system's certificate authorities
(`ca-certificates`); the mail password lives in the database, which only
`zendiloosd` reads, and the API never returns it.

### 4.4 Share and File Permissions

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
  adding fails; the job may run for hours. The files agent walks the tree
  itself, depth first, by descriptor (`O_NOFOLLOW`, only the folders on the
  way down open, each read as it goes, never held whole: a folder of
  millions of entries takes no more memory than a small one), and changes
  each ACL through its descriptor
  (`system.posix_acl_*` via `/proc/self/fd`, the edits of `setfacl -x/-m`
  with the mask recomputed), so a folder swapped for a link meanwhile
  changes nothing. Links, other file systems, what is neither folder nor
  file, and private folders (not root's, only their owner may enter: a
  bin a writer renamed among them) are left alone; an entry that cannot be
  changed, or a folder nested deeper than 256 to look into, does not stop
  the others, the job fails at the end naming one.
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
  bin, deleting is for good. It also restores from the user's own bin:
  back to where an entry was deleted from (`keeptree` keeps the folders),
  without the `Copy #N of ` of a second deletion, folders that are gone
  made again, and a taken name gets a free one beside it; emptying the
  bin deletes `.recycle/<user>` for good.
- **File manager: the kernel checks, not our code.** `zendiloosd` cannot
  read shares itself. Every operation goes to the files agent, which runs
  it on a new thread that first takes on the signed-in user's credentials
  (`setgroups`, `setresgid`, `setresuid` as raw system calls, which on
  Linux change only the calling thread; the thread then has no
  capabilities left, which the agent checks) and ends with the operation.
  Only managed users qualify (UID 2000 to 59999, primary group
  `zendiloos-users`), never root or system accounts. Paths are resolved
  relative to the share's root with
  `openat2(RESOLVE_BENEATH | RESOLVE_NO_XDEV | RESOLVE_NO_MAGICLINKS)`, so
  neither `..` nor a symbolic link leads out of the share; copying and
  removing trees goes name by name from directory descriptors and never
  follows a link. New names (uploads, new folders, renames) must not
  contain control characters, begin or end with a space or end with a
  dot, which Windows could not reach; files written over SMB or NFS keep
  their names and stay reachable. For downloads and uploads the agent passes the opened
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
- **Favourites** are folders an account keeps at hand, in its order (at
  most 50, in the database with the account and the share). Only those in
  shares the account may use are shown; the others are kept, should
  access come back.
- Alternative: the file manager runs with broad rights and checks ACLs
  itself. Simpler, but every gap in the checks becomes a data leak, and new
  files belong to the service account.
- NFS (v4 with `sec=sys`, `root_squash`) trusts the client's UID. Exports
  are therefore restricted to addresses or networks, and the interface
  states the risk. **[Later]** Kerberos.
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
  `/var/lib/zendiloos/setup-code` (0600) and logged to the journal as a
  warning; showing it on the console (HDMI/serial) follows with the
  packaging **[Step 7]**. Nobody on the LAN can complete the setup before
  the owner. The first administrator is created atomically, so two
  concurrent setups cannot both succeed.
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
  `CAP_DAC_READ_SEARCH` (share permissions on files owned by users) and
  `CAP_SETUID`, `CAP_SETGID` (the file manager); `/etc`, `/usr` and `/var`
  read-only; the database, Samba's state, `/etc/zendiloos`, SSH keys and
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
reaches it; the agent does, and only to run apps that pass a policy.

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
catalog's crate `zendiloos-catalog` (pure: text in, problems out). The
agent and `zendiloos-infra` use it as a git dependency pinned to one
commit, so a change to the catalog's checks reaches zendiloOS only when
the pin is moved deliberately: Renovate opens a pull request for each new
commit of the catalog (`renovate.json`), CI checks it, a maintainer merges
it, the next release ships it. The apps themselves are read live from the
index, so new apps need no release. Within `format: 1` the catalog stays
readable by released versions of zendiloOS; what they cannot read gets a
new index (`/v2/`) beside the old one. The policy allows by name: images
following a tag of their developers, bind mounts only from the app's data
folder and the shares the administrator picks, published ports from
variables, no Docker volumes, no `privileged`, no host namespaces; devices
of the kinds the format lists (graphics, VPN, serial, cameras, sound,
Coral TPUs; passed only when the machine has them), extra capabilities
(`NET_ADMIN`, `SYS_NICE`), the host network and the Docker socket only as
permissions the app declares and the administrator confirms. The Docker
socket hands an app control over the whole NAS, and the interface says so;
the agent binds it only for an app whose kept compose file records the
permission, and only when `/run/docker.sock` is root's socket. Generated
secrets take the length and alphabet the catalog asks for (32 letters and
digits otherwise).

**The Docker app** (context `zendiloos-containers`) shows the engine as
it is: every container, image, network and Compose project, whoever made
them; containers and projects made elsewhere are started, stopped,
restarted, updated and removed here, an app of a catalog only started,
stopped and restarted (the App Center changes it, and its Compose file
holds generated settings, so the agent shows it to no one). The agent
runs `docker` with fixed arguments and checked names (`ContainerId`,
`ImageReference`, `ProjectName`, …) and hands back what it printed;
zendiloosd reads it. Container logs are both streams in the order they
were written.

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
the audit log records `risks_accepted`. Deploying (pull, then
`up --detach --remove-orphans`) and removing (`down`; images, data and the
folder stay) are jobs.

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
Center, images pinned by digest never looked at.

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

**Installing** is recorded with the app's values, then a job takes the
steps, each through an agent:

1. *Prepare* (system agent): the dataset `<pool>/zendiloos.apps/<id>` on
   the pool chosen in the store's "choose a location" step (Docker's pool
   unless another is picked; a browser can keep the choice for every
   install), mounted by its own unit at
   `/volume<N>/@apps/<id>` before Docker starts (a parent
   `zendiloos.apps` that is never mounted holds them); and the app's
   account `app-<id>`, a user with a group of its own and IDs never handed
   out before, told from people's accounts by its comment, so no request
   for people or groups touches it. App ids have at most 28 characters
   for that.
2. *Shares*: the Sharing context keeps what each app's account may do on
   which share (`share_app_access`) and writes it into the share's ACL
   beside the people's rules, on everything in the share too; Samba never
   learns of it, and a share an app uses cannot be deleted. Instead of a
   whole share, the administrator may pick a folder in it (`media/Films`,
   from a folder tree in the install dialog); the account's access is
   still the share's, but the container sees the folder only.
3. *Check* (system agent): it reads the manifest and the Compose file
   itself and does what `docs/format.md` section 8 of the catalog
   requires: `check_source` first, then `docker compose config` twice,
   from stdin, with nothing in the environment but the values (setting
   names Docker, Compose, Go or the C library read are refused),
   `--env-file /dev/null` and an empty project folder; `check_variables`
   and the policy on the stand-in form, the policy again on the real one.
   Devices the machine lacks are left out. Only this normalized form is
   kept, readable by root only (`/etc/zendiloos/apps/<id>.json`), and
   ever started.
4. *Folders* (files agent): the bind-mount sources below the data folder,
   made name by name without following a link, as the app's account.
5. *Pull* and *start* (system agent, with a timeout of 30 minutes for
   images): before every start the agent walks each folder an app mounts
   again, refusing links and other file systems on the way.

**Folders in shares are pinned.** A share's root is a dataset's mount
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
change; `SHARE_<ID>` points there, and the bind refuses to start rather
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
test VM, across a reboot (`just vm-app-folders-test`).

The account, `PUID`/`PGID`, the time zone and the data folder come from
the agent, not from `zendiloosd`. Ports are checked against zendiloOS's
own, the other apps' and those something listens on now
(`/proc/net/{tcp,udp}*`). Generated settings are 32 letters and digits,
made once and kept across updates. A failed step leaves the app *failed*:
it is tried again with the same values, or removed. Removing takes the
containers, images and kept file, destroys the dataset with the data, then
takes the account's share access and finally the account. An update is
offered when the app's `digest` in its catalog changes; it keeps the
values, gives new ports their default and new generated settings a value,
and asks again only for permissions beyond the old version's. Verified in
the second test VM (an app with and one without a share, removal, a
restart).

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
| SSH | the SSH server as a service in the control panel (Terminal): on or off (off means stopped and disabled; open sessions stay, as `KillMode=process`), its port (22, or a free one from 1024; zendiloosd refuses a port another program listens on, read from `/proc/net/tcp`), and whether root and passwords may log in, each either set or left as the system has it. The settings go to `/etc/ssh/sshd_config.d/00-zendiloos.conf`, read before the other drop-ins, so they win; `sshd -t` checks them first and the previous file returns if it fails. On request a transient systemd timer turns SSH off again after up to a day. Who may log in is Debian's: the accounts of the interface have no shell | keys, users allowed to log in and a web terminal belong to Debian's configuration or later |
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
